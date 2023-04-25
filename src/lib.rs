use std::future::{Future};
use std::pin::Pin;
use ::std::sync::{Arc, Mutex};
use std::task::{Poll, Context, Waker};
use std::thread::JoinHandle;

use chrono::{DateTime, Duration, TimeZone, Utc};
use itertools::Itertools;

use std::collections::{
    btree_map::BTreeMap, hash_map::HashMap, hash_set::HashSet, vec_deque::VecDeque,
};
use std::hash::{Hash, Hasher};

mod utils;

/// Constraints on start time
#[derive(Copy, Clone, Debug)]
pub struct StartTimeRange {
    /// This is the earliest start time. If None, means there is no earliest time.
    pub earliest_start: Option<DateTime<Utc>>,
    /// This is the latest start time. If None, means there is no earliest time.
    pub latest_start: Option<DateTime<Utc>>,
}

/// Reservation request parameters
#[derive(Clone, Debug)]
pub struct ReservationParameters {
    /// Resource you want to reserve
    pub resource_name: String,
    /// Duration of the reervation. If the duration is none, it is assumed this is an indefinite request.
    /// I.E this reservation is permananent.
    pub duration: Option<Duration>,
    /// Start time constraints.
    pub start_time: StartTimeRange,
}

/// Implements a simple cost function model
pub trait CostFunction {
    /// Givern some parameters and the start time return the cost of a reservation starting at the instant
    fn cost(&self, parameters: &ReservationParameters, instant: &DateTime<Utc>) -> f64;
}

/// This implements a solution handling interface.
pub trait SolutionHandler {
    /// Propose final solution. If this solution is unacceptable at the moment for whatever reason the
    /// fleet adapter should return false. A rollout will not take place.
    fn propose_solution(&self, time: &DateTime<Utc>, duration: Duration, resource: String) -> bool;

    /// Callback is triggered when a solution is claimed, or there is a change in the solution
    fn solution_claimed(&self) -> bool;

    /// Callback is triggered when a solution has been removed. I.E. Robot no longer has access to the resource.
    /// It may also be triggered when there is a change in the solution. This would translate to an unassign call
    /// followed by an assign call
    fn unassign_solution(&self) -> bool;
}


/// Reservation Request represents one possible alternative reservation
#[derive(Clone)]
pub struct ReservationRequest {
    /// The parameters
    pub parameters: ReservationParameters,
    /// Cost function to use
    pub cost_function: Arc<dyn CostFunction + Send + Sync>
}

impl ReservationRequest {
    /// Check if a certain set of parameters satisfies this request
    fn satisfies_request(&self, start_time: &DateTime<Utc>, duration: Option<Duration>) -> bool {
        if let Some(earliest_start_time) = self.parameters.start_time.earliest_start {
            if earliest_start_time > *start_time {
                return false;
            }
        }

        if let Some(latest_start_time) = self.parameters.start_time.latest_start {
            if latest_start_time < *start_time {
                return false;
            }
        }

        duration == self.parameters.duration
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Hash, Eq)]
struct Assignment(usize, usize, Option<Duration>);

/// Represents a single resource's schedule.
#[derive(Clone, Debug, PartialEq, Hash, Eq)]
struct ReservationSchedule {
    schedule: BTreeMap<DateTime<Utc>, Assignment>,
}

enum NextInstant {
    Beginning,
    NextInstant(DateTime<Utc>),
    NoMoreAllowed,
}

#[derive(Clone, Debug, PartialEq, Hash, Eq)]
enum SchedChange {
    Remove(DateTime<Utc>),
    Add(DateTime<Utc>, Assignment),
}

impl ReservationSchedule {
    fn new() -> Self {
        Self {
            schedule: BTreeMap::new(),
        }
    }
    /// Checks consistency
    fn check_consistency(&self) -> bool {
        let mut next_min_instant = NextInstant::Beginning;
        for (instant, assignment) in &self.schedule {
            match next_min_instant {
                NextInstant::NoMoreAllowed => {
                    return false;
                }
                _ => {
                    if let NextInstant::NextInstant(time) = next_min_instant {
                        if *instant < time {
                            return false;
                        }
                    }

                    if let Some(duration) = assignment.2 {
                        next_min_instant = NextInstant::NextInstant(*instant + duration);
                    } else {
                        next_min_instant = NextInstant::NoMoreAllowed;
                    }
                }
            }
        }
        true
    }

    /// Returns a list of assignments that may have a potential conflict with the relevant ReservationRequest
    fn check_potential_conflict(
        &self,
        reservation: &ReservationRequest,
    ) -> std::collections::btree_map::Range<'_, DateTime<Utc>, Assignment> {
        let earliest_time = reservation.parameters.start_time.earliest_start;

        let latest_time = reservation.parameters.start_time.latest_start;

        // Identify potential conflicting range.
        if let Some(latest_time) = latest_time {
            if let Some(earliest_time) = earliest_time {
                let reservations_before = self.schedule.range(..earliest_time);

                // If a reservation has an earliest time, we still need to check the reservation
                // just before it to make sure that the reservation just before does not overlap
                let earliest_time = if let Some((time_res, assignment)) =
                    reservations_before.into_iter().next_back()
                {
                    if let Some(duration) = assignment.2 {
                        if *time_res + duration > earliest_time {
                            *time_res
                        } else {
                            earliest_time
                        }
                    } else {
                        *time_res
                    }
                } else {
                    earliest_time
                };

                if let Some(duration) = reservation.parameters.duration {
                    // If a reservation has a lower bound and upper bound on a starting time
                    // and it has a fixed minimum duration then the range wich conflicts is as follows.
                    self.schedule.range(earliest_time..latest_time + duration)
                } else {
                    // If it only has no duration but has a lower bound, then conflicts will take place
                    // after the earliest time.
                    self.schedule.range(earliest_time..)
                }
            } else if let Some(duration) = reservation.parameters.duration {
                // If it has a duration and only a latest time then conflicts will take place before
                // or up to the latest time
                self.schedule.range(..latest_time + duration)
            } else {
                // If it has a latest time, no duration and no earliest time.
                self.schedule.range(..)
            }
        } else if let Some(earliest_time) = earliest_time {
            let reservations_before = self.schedule.range(..earliest_time);

            // If a reservation has an earliest time, we still need to check the reservation
            // just before it to make sure that the reservation just before does not overlap
            let earliest_time =
                if let Some((time_res, assignment)) = reservations_before.into_iter().next_back() {
                    if let Some(duration) = assignment.2 {
                        if *time_res + duration > earliest_time {
                            *time_res
                        } else {
                            earliest_time
                        }
                    } else {
                        *time_res
                    }
                } else {
                    earliest_time
                };

            self.schedule.range(earliest_time..)
        } else {
            self.schedule.range(..)
        }
    }

    fn get_possible_schedule_changes(
        &self,
        res_idx: usize,
        choice_idx: usize,
        request: &ReservationRequest,
        claimed_requests: &HashSet<usize>
    ) -> Vec<SchedChange> {
        let mut res = vec![];

        let res_range = self.check_potential_conflict(request);

        let mut range_iter_copy = res_range.clone();

        let mut num_potential_conflicts = 0;

        // One possibility is we unassign the item from the schedule.
        // TODO(arjo): Don't do this unless there is a real conflict.
        // I.E: if we can't assign the duration because there is a reservation
        // in the way.
        for (time, assignment) in res_range {
            if claimed_requests.contains(&assignment.0) 
            {
                // Do not reshuffle "claimed" requests
                // TODO(arjo): Consider just deleting such Assignments.
                continue;
            }
            res.push(SchedChange::Remove(*time));
            num_potential_conflicts += 1;
        }

        // Another option is to add the current reservation
        // Get earliest insertion point
        let earliest_insertion_point =
            if let Some(earliest) = request.parameters.start_time.earliest_start {
                earliest
            } else {
                let mut range_iter_copy2 = range_iter_copy.clone();
                if let Some((time, _)) = range_iter_copy2.next() {
                    *time
                } else {
                    Utc::now()
                }
            };

        // Get latest insertion point
        let latest_insertion_point =
            if let Some(latest) = request.parameters.start_time.latest_start {
                latest
            } else {
                let mut range_iter_copy2 = range_iter_copy.clone();
                if let Some((time, assignment)) = range_iter_copy2.next_back() {
                    if let Some(duration) = assignment.2 {
                        *time + duration
                    } else {
                        *time
                    }
                } else {
                    Utc::now()
                }
            };

        // This is easy for indefinite reservations as only one indefinte reservation can be added to the end of a schedule
        if request.parameters.duration == None {
            if let Some((last_req_time, last_req_assignment)) = range_iter_copy.next_back() {
                if let Some(duration) = last_req_assignment.2 {
                    if *last_req_time + duration > earliest_insertion_point
                        && *last_req_time + duration <= latest_insertion_point
                    {
                        res.push(SchedChange::Add(
                            *last_req_time + duration,
                            Assignment(res_idx, choice_idx, None),
                        ));
                    }
                }
            } else {
                // TODO(arjo): guess best insertion point.
                // For now try to assign it to latest possible time so that other reservations can be serviced.
                res.push(SchedChange::Add(
                    latest_insertion_point,
                    Assignment(res_idx, choice_idx, None),
                ));
            }
        }
        if let Some(req_dur) = request.parameters.duration {
            if num_potential_conflicts == 0 {
                res.push(SchedChange::Add(
                    earliest_insertion_point,
                    Assignment(res_idx, choice_idx, request.parameters.duration),
                ));
            } else {
                for ((res1_time, res1_dur), (res2_time, _)) in
                    range_iter_copy.clone().tuple_windows()
                {
                    // TODO(arjo): Remove unwrap
                    let last_end = *res1_time
                        + res1_dur.2.expect(
                            "Expect all reservations before the last one to have a duration.",
                        );
                    let time_gap = *res2_time - last_end;

                    if time_gap >= req_dur && request.satisfies_request(&last_end, Some(req_dur)) {
                        // Fit reservation in.
                        res.push(SchedChange::Add(
                            last_end,
                            Assignment(res_idx, choice_idx, Some(req_dur)),
                        ))
                    }
                }

                // Try before the first reservation
                if let Some((first_reservation_start_time, _)) = range_iter_copy.clone().next() {
                    let squeezed_before = *first_reservation_start_time - req_dur;
                    if request.satisfies_request(&squeezed_before, Some(req_dur)) {
                        res.push(SchedChange::Add(
                            squeezed_before,
                            Assignment(res_idx, choice_idx, Some(req_dur)),
                        ))
                    }
                    let earliest_end = earliest_insertion_point + req_dur;
                    if earliest_end < *first_reservation_start_time
                        && request.satisfies_request(&earliest_insertion_point, Some(req_dur))
                    {
                        res.push(SchedChange::Add(
                            earliest_insertion_point,
                            Assignment(res_idx, choice_idx, Some(req_dur)),
                        ));
                    }
                } else {
                    // There's nothing just insert the earliest reservation and call it a day.
                    res.push(SchedChange::Add(
                        earliest_insertion_point,
                        Assignment(res_idx, choice_idx, Some(req_dur)),
                    ));
                }

                if let Some((last_reservation_start_time, last_reservation_details)) =
                    range_iter_copy.clone().next_back()
                {
                    if let Some(duration) = last_reservation_details.2 {
                        let last_end_time = *last_reservation_start_time + duration;
                        if request.satisfies_request(&last_end_time, Some(req_dur)) {
                            res.push(SchedChange::Add(
                                last_end_time,
                                Assignment(res_idx, choice_idx, Some(req_dur)),
                            ));
                        }
                    }
                }
            }
        }

        res
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReservationState {
    unassigned: HashSet<usize>,
    assigned: HashMap<usize, (String, DateTime<Utc>)>,
    assignments: HashMap<String, ReservationSchedule>,
}

impl Hash for ReservationState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for f in &self.unassigned {
            f.hash(state);
        }

        for f in &self.assigned {
            f.hash(state);
        }

        for f in &self.assignments {
            f.hash(state);
        }
    }
}

impl ReservationState {
    fn create_empty() -> Self {
        Self {
            unassigned: HashSet::new(),
            assigned: HashMap::new(),
            assignments: HashMap::new(),
        }
    }

    fn create_unassign_state(&self, time: DateTime<Utc>, resource: &String) -> Self {
        let mut new_self = self.clone(); // TODO(arjo): Implement views via traits.
        if let Some(schedule) = new_self.assignments.get_mut(resource) {
            if let Some((_, assignment)) = schedule.schedule.remove_entry(&time) {
                new_self.assigned.remove(&assignment.0);
                new_self.unassigned.insert(assignment.0);
            }
        }
        new_self
    }

    fn create_assign_state(
        &self,
        time: DateTime<Utc>,
        assignment: Assignment,
        resource: &String,
    ) -> Self {
        let mut new_self = self.clone(); // TODO(arjo): Implement views via traits.
        new_self.unassigned.remove(&assignment.0);
        new_self.assigned.insert(assignment.0, (resource.clone(), time));
        new_self
            .assignments
            .get_mut(resource)
            .expect("Reservation Schedule should exist before hand")
            .schedule
            .insert(time, assignment);
        new_self
    }

    fn create_new_with_resources(resources: &Vec<String>) -> Self {
        let mut state = Self::create_empty();
        for resource in resources {
            state
                .assignments
                .insert(resource.clone(), ReservationSchedule::new());
        }
        state
    }

    fn attempt_service_req(
        &self,
        res_idx: usize,
        choice_idx: usize,
        req: &ReservationRequest,
        claimed_requests: &HashSet<usize>
    ) -> Vec<(Self, SchedChange)> {
        let mut results = vec![];
        let changes = if let Some(schedule) = self.assignments.get(&req.parameters.resource_name) {
            schedule.get_possible_schedule_changes(res_idx, choice_idx, &req, claimed_requests)
        } else {
            // Implicitly add resource
            // TODO(arjo): Create resource configuration struct
            /*self.assignments.insert(req.parameters.resource_name.clone(), ReservationSchedule::new());
            self.assignments[&req.parameters.resource_name].get_possible_schedule_changes(res_idx, choice_idx, &req)*/
            vec![]
        };

        for change in changes {
            let new_state = match change {
                SchedChange::Add(time, assignment) => {
                    self.create_assign_state(time, assignment, &req.parameters.resource_name)
                }
                SchedChange::Remove(time) => {
                    self.create_unassign_state(time, &req.parameters.resource_name)
                }
            };

            // TODO(arjo): Remove this panic
            if self.unassigned.len() + self.assigned.len()
                != new_state.unassigned.len() + new_state.assigned.len()
            {
                panic!(
                    "From {:?} got {:?} for {:?} which resulted in an inconsistent state: {:?}",
                    self, change, req.parameters.resource_name, new_state
                );
            }
            results.push((new_state, change));
        }

        results
    }
}

/// The main reservation system.
pub struct SyncReservationSystem {
    reservation_queue: Vec<Vec<ReservationRequest>>,
    current_state: ReservationState,
    claimed_requests: HashSet<usize>,
    cummulative_cost: f64,
}

impl SyncReservationSystem {
    pub fn create_new_with_resources(resources: &Vec<String>) -> Self {
        Self {
            reservation_queue: vec![],
            current_state: ReservationState::create_new_with_resources(resources),
            claimed_requests: HashSet::new(),
            cummulative_cost: 0f64
        }
    }

    fn next_states(&self, reservation_state: &ReservationState) -> Vec<(ReservationState, SchedChange)> {
        let mut result = vec![];
        for res in &reservation_state.unassigned {
            // Attempt each reservation alternative
            for choice_idx in 0..self.reservation_queue[*res].len() {
                let mut states = reservation_state.attempt_service_req(
                    *res,
                    choice_idx,
                    &self.reservation_queue[*res][choice_idx],
                    &self.claimed_requests
                );
                result.append(&mut states);
            }
        }
        result
    }

    fn search_for_solution(&self) -> Vec<(ReservationState,  Vec<SchedChange>)> {
        // TODO remove HashSet. Move to view based method
        let mut explored = HashSet::new();
        let mut results = vec![];
        let mut parent: HashMap<ReservationState, (ReservationState, SchedChange)> = HashMap::new();

        let mut queue = VecDeque::new();
        queue.push_back(self.current_state.clone());
        explored.insert(self.current_state.clone());

        while let Some(state) = queue.pop_front() {
            let next_states = self.next_states(&state);
            for (next_state, change) in next_states {
                if explored.contains(&next_state) {
                    continue;
                }
                if next_state.unassigned.len() == 0 {
                    // Retrace parent
                    let mut cursor = state.clone();
                    let mut rollout_plan = vec!();
                    rollout_plan.push(change.clone());
                    while cursor != self.current_state {
                        if let Some((state, change)) = parent.get(&cursor) {
                            rollout_plan.push(change.clone());
                            cursor = state.clone();
                        }
                        else {
                            panic!("Should never get here");
                        }
                    }
                    results.push((next_state.clone(), rollout_plan));
                }
                explored.insert(next_state.clone());
                queue.push_back(next_state.clone());
                parent.insert(next_state, (state.clone(), change));
            }
        }

        results
    }

    fn search_for_solution_to_problem(&mut self, reservation: usize) -> Option<(ReservationState, Vec<SchedChange>)> {
        let mut explored = HashSet::new();
        let mut parent: HashMap<ReservationState, (ReservationState, SchedChange)> = HashMap::new();

        let mut queue = VecDeque::new();
        queue.push_back(self.current_state.clone());
        explored.insert(self.current_state.clone());

        while let Some(state) = queue.pop_front() {
            let next_states = self.next_states(&state);
            for (next_state, change) in next_states {
                if explored.contains(&next_state) {
                    continue;
                }
                if next_state.assigned.contains_key(&reservation) {
                    // Retrace parent
                    let mut cursor = state.clone();
                    let mut rollout_plan = vec!();
                    rollout_plan.push(change.clone());
                    while cursor != self.current_state {
                        if let Some((state, change)) = parent.get(&cursor) {
                            rollout_plan.push(change.clone());
                            cursor = state.clone();
                        }
                        else {
                            panic!("Should never get here");
                        }
                    }
                    return Some((next_state.clone(), rollout_plan));
                }
                explored.insert(next_state.clone());
                queue.push_back(next_state.clone());
                parent.insert(next_state, (state.clone(), change));
            }
        }

        None
    }

    // Forcing a reservation has a much simpler target condition - 
    // as long as the forced reservation has a simple solution, the system will return a consistent
    // the easiest way to satisfy a reservation. 
    pub fn force_reservation(&mut self, reservation: usize) -> bool {
        if let Some((state, _changes)) = self.search_for_solution_to_problem(reservation) {
            self.current_state = state;
            return true;
        }
        false
    }

    pub fn request_reservation(&mut self, reservations: Vec<ReservationRequest>) -> usize {
        self.reservation_queue.push(reservations);
        let request_handle = self.reservation_queue.len() - 1;
        self.current_state
            .unassigned
            .insert(self.reservation_queue.len() - 1);
        let mut soln = self.search_for_solution();
        if soln.len() > 0 {
            // TODO(arjo) find best solution
            soln.sort_by(|a, b| { a.1.len().partial_cmp(&b.1.len()).unwrap()});

            // Compute changes

            for sched_change in &soln[0].1 {
                match sched_change {
                    SchedChange::Add(res, assignment) => {

                    },
                    SchedChange::Remove(time) => {

                    }
                }
            }
            self.current_state = soln[0].0.clone();
        }
        request_handle
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct ReservationVoucher {
    index: usize
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
enum VoucherState {
    OnQueue,
    Solved(usize),
    Claimed
}

pub enum RequestError {
    NonExistantResource(String)
}

enum Action {
    Add(ReservationVoucher, Vec<ReservationRequest>),
    Cancel(ReservationVoucher),
    Exit
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum ClaimError {
    InvalidVoucher,
    OutOfTimeBounds,
    VoucherClaimedAlready,
    ResourcesTooBusy,
    Other(String)
}

pub struct ClaimResult {
    /*reservation_system: Arc<Mutex<SyncReservationSystem>>,
    ticket_state: Arc<Mutex<HashMap<ReservationVoucher, VoucherState>>>,
    ticket_wakers: Arc<Mutex<HashMap<ReservationVoucher, Waker>>>,
    voucher: ReservationVoucher,*/
    voucher_context: Arc<Mutex<VoucherContext>>,
    voucher: ReservationVoucher,
}

impl Future for ClaimResult {
    type Output = Result<String, ClaimError>;

    fn poll(self: Pin<&mut ClaimResult>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut voucher_ctx = self.voucher_context.lock().unwrap();

        let ticket_state: Result<VoucherState, ClaimError> = 
            if let Some(ticket_state) = voucher_ctx.ticket_state.get(&self.voucher) {
                Ok(*ticket_state)
            }
            else{          
                Err(ClaimError::InvalidVoucher)
            };

        if let Err(err) = ticket_state {
            return Poll::Ready(Err(err));
        }

        match ticket_state.unwrap() {
            VoucherState::Claimed => {
                Poll::Ready(Err(ClaimError::VoucherClaimedAlready))
            },
            VoucherState::OnQueue => {
                if let Some(_) = voucher_ctx.ticket_wakers.get(&self.voucher)
                {
                    Poll::Ready(Err(ClaimError::VoucherClaimedAlready))
                }
                else
                {
                    voucher_ctx.ticket_wakers.insert(self.voucher.clone(), cx.waker().clone());
                    Poll::Pending
                }
            },
            VoucherState::Solved(id) => {
                // Check if within time limit
                voucher_ctx.reservation_system.claimed_requests.insert(id);
                if let Some((resource, _start_time)) = voucher_ctx.reservation_system.current_state.assigned.get(&id) {
                    let res = resource.clone();
                    voucher_ctx.ticket_state.insert(self.voucher.clone(), VoucherState::Claimed);
                    Poll::Ready(Ok(res))
                }
                else {
                    voucher_ctx.reservation_system.force_reservation(id);
                    if let Some((resource, _start_time)) = voucher_ctx.reservation_system.current_state.assigned.get(&id) {
                        let res = resource.clone();
                        voucher_ctx.ticket_state.insert(self.voucher.clone(), VoucherState::Claimed);
                        Poll::Ready(Ok(res))
                    }
                    else {
                        Poll::Ready(Err(ClaimError::ResourcesTooBusy))
                    }
                }
            }
        }
    }
}

struct VoucherContext {
    reservation_system: SyncReservationSystem,
    ticket_state: HashMap<ReservationVoucher, VoucherState>,
    ticket_wakers: HashMap<ReservationVoucher, Waker>,
    voucher_max_idx: usize
}

impl VoucherContext {
    fn new(resources: &Vec<String>) -> Self {
        Self {
            reservation_system: SyncReservationSystem::create_new_with_resources(resources),
            ticket_state: HashMap::new(),
            ticket_wakers: HashMap::new(),
            voucher_max_idx: 0
        }
    }
}

pub struct AsyncReservationSystem {
    //reservation_system: Arc<Mutex<SyncReservationSystem>>,
    //voucher_max_idx: usize,
    work_queue: Arc<utils::queue::WorkQueue<Action>>,
    //ticket_state: Arc<Mutex<HashMap<ReservationVoucher, VoucherState>>>,
    //ticket_wakers: Arc<Mutex<HashMap<ReservationVoucher, Waker>>>,
    resources: HashSet<String>,
    voucher_context: Arc<Mutex<VoucherContext>>
}

impl AsyncReservationSystem {

    pub fn new(resources: &Vec<String>) -> Self {
        Self {
            /*reservation_system: Arc::new(Mutex::new(SyncReservationSystem::create_new_with_resources(resources))),
            voucher_max_idx: 0usize,
            work_queue: Arc::new(utils::queue::WorkQueue::new()),
            ticket_state: Arc::new(Mutex::new(HashMap::new())),
            ticket_wakers: Arc::new(Mutex::new(HashMap::new())),*/
            voucher_context: Arc::new(Mutex::new(VoucherContext::new(resources))),
            work_queue: Arc::new(utils::queue::WorkQueue::new()),
            resources: {
                let mut hashset = HashSet::new();
                for resource in resources {
                    hashset.insert(resource.clone());
                }
                hashset
            }
        }
    }

    pub fn request_reservation(&mut self, alternatives: Vec<ReservationRequest>) -> Result<ReservationVoucher, RequestError> {
        for alternative in &alternatives {
            if !self.resources.contains(&alternative.parameters.resource_name) {
                return Err(RequestError::NonExistantResource(alternative.parameters.resource_name.clone()));
            }
        }

        let mut voucher_context = self.voucher_context.lock().unwrap();
        
        voucher_context.voucher_max_idx += 1;
        let ticket = ReservationVoucher { index: voucher_context.voucher_max_idx - 1 };
        self.work_queue.push(Action::Add(ticket.clone(), alternatives));
        voucher_context.ticket_state.insert(ticket.clone(), VoucherState::OnQueue);
        Ok(ticket)
    }

    pub fn spin_in_bg(&self) -> JoinHandle<()> {
        let voucher_context= self.voucher_context.clone();
        let work_queue = self.work_queue.clone();
        
        std::thread::spawn(move || {
            while let action = work_queue.wait_for_work() {
                match action {
                    Action::Add(voucher, parameters) => {
                        let mut ctx: std::sync::MutexGuard<VoucherContext> = voucher_context.lock().expect("Unable to lock voucher context");
                        let index = ctx.reservation_system.request_reservation(parameters);
                        ctx.ticket_state.insert(voucher.clone(), VoucherState::Solved(index));
                        if let Some(waker) = ctx.ticket_wakers.remove(&voucher) {
                            waker.wake();
                        }
                    },
                    Action::Cancel(voucher) => {
                        todo!("Cancellation unsupported");
                    },
                    Action::Exit => {
                        break;
                    }
                }
            }
        })
    }

    /// Return relevant resource.
    pub fn claim(&self, voucher: ReservationVoucher) -> ClaimResult {
        ClaimResult { 
            /*reservation_system: self.reservation_system.clone(), 
            ticket_state: self.ticket_state.clone(), 
            ticket_wakers: self.ticket_wakers.clone(), */
            voucher_context: self.voucher_context.clone(),
            voucher
        }
    }
}

struct NoCost {}

impl CostFunction for NoCost {
    fn cost(&self, _parameters: &ReservationParameters, _instant: &DateTime<Utc>) -> f64 {
        0f64
    }
}

#[cfg(test)]
#[test]
fn test_satisfies_request() {
    let req = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "test".to_string(),
            duration: Some(Duration::minutes(30)),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: None,
            },
        },
        cost_function: Arc::new(NoCost {}),
    };

    let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap();
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(30))));
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(20))) == false);
    assert!(req.satisfies_request(&start_time, None) == false);

    let req = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "test".to_string(),
            duration: Some(Duration::minutes(30)),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 8, 10, 11).unwrap()),
            },
        },
        cost_function: Arc::new(NoCost {}),
    };

    // Too early
    let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 5, 10, 11).unwrap();
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(30))) == false);

    // Too late
    let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 10, 10, 11).unwrap();
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(30))) == false);

    // OK
    let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap();
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(30))));
}

#[cfg(test)]
#[test]
fn test_check_consistency() {
    let mut sched = ReservationSchedule {
        schedule: BTreeMap::new(),
    };
    // Empty schedule should be consistent
    assert!(sched.check_consistency());

    // Create a schedule with no overlapping reservations
    sched.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap(),
        Assignment(0usize, 0usize, Some(Duration::minutes(40))),
    );

    sched.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 9, 10, 11).unwrap(),
        Assignment(0usize, 0usize, None),
    );

    assert!(sched.check_consistency());

    // Add yet another reservation after the indefinite reservation
    sched.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 12, 10, 11).unwrap(),
        Assignment(0usize, 0usize, Some(Duration::minutes(40))),
    );
    assert!(!sched.check_consistency());

    // Cler and create a schedule with conflicts
    sched.schedule.clear();
    sched.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap(),
        Assignment(0usize, 0usize, Some(Duration::minutes(40))),
    );

    sched.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 6, 15, 11).unwrap(),
        Assignment(0usize, 0usize, None),
    );

    assert!(!sched.check_consistency());
}

#[cfg(test)]
#[test]
fn test_conflict_checker() {
    let cost_func = Arc::new(NoCost {});

    // Check for an indefinite reservation with no specification on
    let indefinite_no_constraints = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource1".to_string(),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: None,
            },
            duration: None,
        },
        cost_function: cost_func.clone(),
    };

    let indefinite_with_constraints = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource1".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: None,
        },
        cost_function: cost_func.clone(),
    };

    // Definite requet with fixed time bound
    let definite_request_starting_with_specified_start_time = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource1".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: cost_func.clone(),
    };

    let definite_request_with_no_earliest = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource1".to_string(),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30))
        },
        cost_function: cost_func.clone(),
    };

    let definite_request_with_no_latest = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource1".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: None,
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: cost_func.clone(),
    };

    let mut reservation_schedule = ReservationSchedule {
        schedule: BTreeMap::new(),
    };

    let res = reservation_schedule.check_potential_conflict(&indefinite_no_constraints);
    assert_eq!(res.count(), 0);

    // Add an indefinite reservation to the schedule.
    reservation_schedule.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 9, 10, 11).unwrap(),
        Assignment(0usize, 0usize, None),
    );

    let res = reservation_schedule.check_potential_conflict(&indefinite_no_constraints);
    assert_eq!(res.count(), 1);

    let res = reservation_schedule.check_potential_conflict(&indefinite_with_constraints);
    assert_eq!(res.count(), 1);

    let res = reservation_schedule
        .check_potential_conflict(&definite_request_starting_with_specified_start_time);
    assert_eq!(res.count(), 0);

    // The latest end time is before the last reservation so there should be no conflict
    let res = reservation_schedule.check_potential_conflict(&definite_request_with_no_earliest);
    assert_eq!(res.count(), 0);

    // Since no latest time it could conflict with the last reservation
    let res = reservation_schedule.check_potential_conflict(&definite_request_with_no_latest);
    assert_eq!(res.count(), 1);

    // Clear schedule
    let mut reservation_schedule = ReservationSchedule {
        schedule: BTreeMap::new(),
    };

    // Empty schedule no conflicts
    let res = reservation_schedule.check_potential_conflict(&indefinite_no_constraints);
    assert_eq!(res.count(), 0);

    // Add a reservation
    reservation_schedule.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 2, 10, 11).unwrap(),
        Assignment(0usize, 0usize, Some(Duration::minutes(30))),
    );

    // Add a request
    let res = reservation_schedule
        .check_potential_conflict(&definite_request_starting_with_specified_start_time);
    assert_eq!(res.count(), 0);

    // Insert a potentially overlapping reservation
    reservation_schedule.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 5, 10, 11).unwrap(),
        Assignment(0usize, 0usize, Some(Duration::minutes(90))),
    );

    let res = reservation_schedule
        .check_potential_conflict(&definite_request_starting_with_specified_start_time);
    assert_eq!(res.count(), 1);
}

#[cfg(test)]
#[test]
fn test_branching_operations() {
    let mut reservation_schedule = ReservationSchedule {
        schedule: BTreeMap::new(),
    };

    reservation_schedule.schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 5, 10, 11).unwrap(),
        Assignment(0usize, 0usize, Some(Duration::minutes(90))),
    );

    let request = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource1".to_string(),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(NoCost {}),
    };

    let changes = reservation_schedule.get_possible_schedule_changes(0, 0, &request, &HashSet::new());
    let num_removed = changes
        .iter()
        .filter(|sched_change| matches!(sched_change, SchedChange::Remove(_)))
        .count();
    assert_eq!(num_removed, 1usize);
    let num_added = changes
        .iter()
        .filter(|sched_change| matches!(sched_change, SchedChange::Add(_, _)))
        .count();
    assert!(num_added > 0usize);

    for change in changes.iter() {
        if let SchedChange::Add(time, assignment) = change {
            let mut new_schedule = reservation_schedule.clone();
            new_schedule.schedule.insert(*time, *assignment);
            assert!(new_schedule.check_consistency());
            assert!(request.satisfies_request(time, assignment.2));
        }
    }
}


#[cfg(test)]
#[test]
fn test_unassign() {
    let time = Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap();
    let state = ReservationState {
        unassigned: HashSet::from_iter(vec![1]),
        assigned: HashMap::from_iter([(0usize, ("station1".to_string(), time))]),
        assignments: HashMap::from_iter([
            ("station2".to_string(), ReservationSchedule::new()),
            (
                "station1".to_string(),
                ReservationSchedule {
                    schedule: BTreeMap::from_iter([(
                        time,
                        Assignment(0, 0, Some(Duration::minutes(30))),
                    )]),
                },
            ),
        ]),
    };

    let unassigned = state.create_unassign_state(time, &"station1".to_string());

    assert_eq!(unassigned.unassigned.len(), 2);
    assert_eq!(unassigned.assigned.len(), 0);

    assert_eq!(
        unassigned.assignments[&"station2".to_string()]
            .schedule
            .len(),
        0
    );
    assert_eq!(
        unassigned.assignments[&"station1".to_string()]
            .schedule
            .len(),
        0
    );
}

#[cfg(test)]
#[test]
fn test_simple_reservation() {
    let resources = vec!["station1".to_string(), "station2".to_string()];
    let mut reservation_system = SyncReservationSystem::create_new_with_resources(&resources);

    let alternative1 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "station1".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(NoCost {}),
    };

    let simple_request = vec![alternative1.clone()];

    assert!(
        reservation_system.current_state.assignments["station1"]
            .schedule
            .len()
            == 0
    );
    reservation_system.request_reservation(simple_request);
    assert!(
        reservation_system.current_state.assignments["station1"]
            .schedule
            .len()
            == 1
    );

    let alternative2 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "station2".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(NoCost {}),
    };

    let multiple_alternatives = vec![alternative1.clone(), alternative2];
    reservation_system.request_reservation(multiple_alternatives);
    assert!(reservation_system.current_state.assignments["station1"].check_consistency());
    assert!(reservation_system.current_state.assignments["station2"].check_consistency());

    let multiple_alternatives = vec![alternative1];
    reservation_system.request_reservation(multiple_alternatives);
    assert!(reservation_system.current_state.assignments["station1"].check_consistency());
    assert!(reservation_system.current_state.assignments["station2"].check_consistency());
}

#[cfg(test)]
#[test]
fn test_async_reservation() {
    use futures::executor::block_on;

    let resources = vec!["station1".to_string(), "station2".to_string()];
    let mut reservation_system = AsyncReservationSystem::new(&resources);
    let res_sys_thread = reservation_system.spin_in_bg();

    let alternative1 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "station1".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(NoCost {}),
    };

    let alternatives = vec![alternative1.clone()];

    let voucher = reservation_system.request_reservation(alternatives);
    if let Ok(voucher) = voucher {
        let result = block_on(reservation_system.claim(voucher));
        assert!(matches!(result, Ok(_)));
    }
    else
    {
        // Expect a valid reservation ticket.
        assert!(false);
    }

    reservation_system.work_queue.push(Action::Exit);
    res_sys_thread.join();
}
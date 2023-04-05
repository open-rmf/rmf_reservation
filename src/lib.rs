use ::std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use itertools::Itertools;

use std::collections::{btree_map::BTreeMap, hash_map::HashMap, hash_set::HashSet, vec_deque::VecDeque};
use std::hash::{Hash, Hasher};

#[derive(Copy, Clone, Debug)]
pub struct StartTimeRange {
    earliest_start: Option<DateTime<Utc>>,
    latest_start: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct ReservationParameters {
    resource_name: String,
    duration: Option<Duration>,
    start_time: StartTimeRange,
}

pub trait CostFunction {
    fn cost(&self, parameters: &ReservationParameters, instant: &DateTime<Utc>) -> f64;
}

#[derive(Clone)]
pub struct ReservationRequest {
    parameters: ReservationParameters,
    cost_function: Arc<dyn CostFunction>,
}

impl ReservationRequest {
    fn satisfies_request(&self, start_time: &DateTime<Utc>, duration: Option<Duration>) -> bool
    {
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

struct PotentialAssignment {
    resource: String,
    start: DateTime<Utc>,
    duration: Duration,
    cost: f64,
}

impl ReservationRequest {
    fn sample(
        _intervals: Duration,
        _earliest_start: Option<DateTime<Utc>>,
        _latest_start: Option<DateTime<Utc>>,
    ) -> Vec<PotentialAssignment> {
        vec![]
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
    Add(DateTime<Utc>, Assignment)
}

impl ReservationSchedule {

    fn new() -> Self {
        Self { schedule: BTreeMap::new() }
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
    /// TODO: Change to return range
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

    fn get_possible_schedule_changes(&self, res_idx: usize, choice_idx: usize, request: &ReservationRequest) -> Vec<SchedChange>
    {
        let mut res = vec!();

        let res_range = self.check_potential_conflict(request);

        let mut range_iter_copy = res_range.clone();

        let mut num_potential_conflicts = 0;
       
        // One possibility is we unassign the item from the schedule.
        // TODO(arjo): Don't do this unless there is a real conflict.
        // I.E: if we can't assign the duration because there is a reservation
        // in the way.
        for (time, assignment) in res_range {
            res.push(SchedChange::Remove(*time));
            num_potential_conflicts += 1;
        }

        // Another option is to add the current reservation
        // Get earliest insertion point
        let earliest_insertion_point = if let Some(earliest) = request.parameters.start_time.earliest_start {
            earliest
        }
        else {
            let mut range_iter_copy2 = range_iter_copy.clone();
            if let Some((time, _)) = range_iter_copy2.next() {
                *time
            } else {
                Utc::now()
            }
        };

        // Get latest insertion point
        let latest_insertion_point = if let Some(latest) = request.parameters.start_time.latest_start {
            latest
        }
        else {
            let mut range_iter_copy2 = range_iter_copy.clone();
            if let Some((time, assignment)) = range_iter_copy2.next_back() {
                if let Some(duration) = assignment.2 {
                    *time + duration
                }
                else  {
                    *time
                }
            } else {
                Utc::now()
            }
        };


        // This is easy for indefinite reservations as only one indefinte reservation can be added to the end of a schedule
        if request.parameters.duration == None {
            if let Some((last_req_time, last_req_assignment)) = range_iter_copy.next_back(){
                if let Some(duration) = last_req_assignment.2 {
                    if *last_req_time + duration > earliest_insertion_point && *last_req_time + duration <= latest_insertion_point {
                        res.push(SchedChange::Add(*last_req_time + duration, Assignment(res_idx, choice_idx, None)));
                    }
                }
            }
            else
            {
                //guess best insertion point. For now try to assign it to latest possible time so that other reservations can be serviced.
                res.push(SchedChange::Add(latest_insertion_point, Assignment(res_idx, choice_idx, None)));
            }
        }
        if let Some(req_dur) = request.parameters.duration
        {
            if num_potential_conflicts == 0 {
                res.push(SchedChange::Add(earliest_insertion_point, Assignment(res_idx, choice_idx, request.parameters.duration)));
            }
            else {
                for ((res1_time, res1_dur), (res2_time, _)) in range_iter_copy.clone().tuple_windows() {
                    
                    // TODO(arjo): Remove unwrap
                    let last_end = *res1_time + res1_dur.2.expect("Expect all reservations before the last one to have a duration.");
                    let time_gap = *res2_time - last_end;

                    if time_gap >= req_dur && request.satisfies_request(&last_end, Some(req_dur)) {
                        // Fit reservation in.
                        res.push(SchedChange::Add(last_end, Assignment(res_idx, choice_idx, Some(req_dur))))
                    }
                }

                // Try before the first reservation
                if let Some((first_reservation_start_time, _)) = range_iter_copy.clone().next() {
                    let squeezed_before = *first_reservation_start_time - req_dur;
                    if request.satisfies_request(&squeezed_before, Some(req_dur))
                    {
                        res.push(SchedChange::Add(squeezed_before, Assignment(res_idx, choice_idx, Some(req_dur))))
                    }
                    let earliest_end = earliest_insertion_point + req_dur;
                    if earliest_end < *first_reservation_start_time && request.satisfies_request(&earliest_insertion_point, Some(req_dur)) {
                        res.push(SchedChange::Add(earliest_insertion_point, Assignment(res_idx, choice_idx, Some(req_dur))));
                    }
                }
                else {
                    // There's nothing just insert the earliest reservation and call it a day.
                    res.push(SchedChange::Add(earliest_insertion_point, Assignment(res_idx, choice_idx, Some(req_dur))));
                }
                

                if let Some((last_reservation_start_time, last_reservation_details)) = range_iter_copy.clone().next_back() {
                    if let Some(duration) = last_reservation_details.2 {
                        let last_end_time =  *last_reservation_start_time + duration;
                        if request.satisfies_request(&last_end_time, Some(req_dur)) {
                            res.push(SchedChange::Add(last_end_time,  Assignment(res_idx, choice_idx, Some(req_dur))));
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
    assigned: HashMap<usize, String>,
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
        Self { unassigned: HashSet::new(), assigned: HashMap::new(), assignments: HashMap::new() }
    }

    fn create_unassign_state(&self, time: DateTime<Utc>, resource: &String) -> Self {
        let mut new_self = self.clone(); // TODO(arjo): Implement views via traits.
        if let Some(schedule)= new_self.assignments.get_mut(resource) {
        if let Some((_, assignment)) = schedule.schedule.remove_entry(&time) {
            new_self.assigned.remove(&assignment.0);
            new_self.unassigned.insert(assignment.0);
        }}
        new_self
    }

    fn create_assign_state(&self, time: DateTime<Utc>, assignment: Assignment, resource: &String) -> Self {
        let mut new_self = self.clone(); // TODO(arjo): Implement views via traits.
        new_self.unassigned.remove(&assignment.0);
        new_self.assigned.insert(assignment.0, resource.clone());
        new_self.assignments.get_mut(resource).expect("Reservation Schedule should exist before hand").schedule.insert(time, assignment);
        new_self
    }

    fn create_new_with_resources(resources: &Vec<String>) -> Self {
        let mut state = Self::create_empty();
        for resource in resources {
            state.assignments.insert(resource.clone(), ReservationSchedule::new());
        }
        state
    }

    fn attempt_service_req(&self, res_idx: usize, choice_idx: usize, req: &ReservationRequest) -> Vec<Self> {
        
        let mut results = vec!();
        let changes = if let Some(schedule) = self.assignments.get(&req.parameters.resource_name) {
            schedule.get_possible_schedule_changes(res_idx, choice_idx, &req)
        }
        else {
            // Implicitly add resource
            // TODO(arjo): Create resource configuration struct
            /*self.assignments.insert(req.parameters.resource_name.clone(), ReservationSchedule::new());
            self.assignments[&req.parameters.resource_name].get_possible_schedule_changes(res_idx, choice_idx, &req)*/
            vec!()
        };
        
        for change in changes {
            let new_state = match change {
                SchedChange::Add(time, assignment) => {
                    self.create_assign_state(time, assignment, &req.parameters.resource_name)
                },
                SchedChange::Remove(time) => {
                    self.create_unassign_state(time, &req.parameters.resource_name)
                }
            };
            
            // TODO(arjo): Remove this panic
            if (self.unassigned.len() + self.assigned.len() != new_state.unassigned.len() + new_state.assigned.len()) 
            {
                panic!("From {:?} got {:?} for {:?} which resulted in an inconsistent state: {:?}", self, change, req.parameters.resource_name, new_state);
            }
            results.push(new_state);
        }

        results
    }
}

pub struct SyncReservationSystem {
    reservation_queue: Vec<Vec<ReservationRequest>>,
    current_state: ReservationState,
    cummulative_cost: f64,
}

impl SyncReservationSystem {
    
    pub fn create_new_with_resources(resources: &Vec<String>) -> Self {
        Self {
            reservation_queue: vec![],
            current_state: ReservationState::create_new_with_resources(resources),
            cummulative_cost: 0f64,
        }
    }

    fn next_states(&self, reservation_state: &ReservationState) -> Vec<ReservationState> {
        let mut result = vec!();
        for res in &reservation_state.unassigned {
            // Attempt each reservation alternative
            for choice_idx in 0..self.reservation_queue[*res].len() {
                let mut states = reservation_state.attempt_service_req(*res, choice_idx, &self.reservation_queue[*res][choice_idx]);
                result.append(&mut states);
            }
        }
        result
    }

    fn search_for_solution(&self) -> Vec<ReservationState> {

        // TODO remove HashSet. Move to view based method
        let mut explored = HashSet::new();
        let mut results = vec!();

        let mut queue = VecDeque::new();
        queue.push_back(self.current_state.clone());
        explored.insert(self.current_state.clone());

        while let Some(state) = queue.pop_back(){
            let next_states = self.next_states(&state);
            for state in next_states {
                if explored.contains(&state) {
                    continue;
                }
                if state.unassigned.len() == 0 {
                    results.push(state.clone());
                }
                explored.insert(state.clone());
                queue.push_back(state);
            }
        }

        results
    }

    pub fn request_reservation(&mut self, reservations: Vec<ReservationRequest>) {
        self.reservation_queue.push(reservations);
        self.current_state.unassigned.insert(self.reservation_queue.len() - 1);
        let soln = self.search_for_solution();
        if soln.len() > 0 {
            self.current_state = soln[0].clone();
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
            start_time: StartTimeRange { earliest_start: None, latest_start: None } },
        cost_function: Arc::new(NoCost{})
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
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 8, 10, 11).unwrap()) 
            }
        },
        cost_function: Arc::new(NoCost{})
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
            duration: Some(Duration::minutes(30)),
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
        cost_function: Arc::new(NoCost{}),
    };

    let changes = reservation_schedule.get_possible_schedule_changes(0, 0, &request);
    let num_removed = changes.iter().filter(|sched_change| matches!(sched_change, SchedChange::Remove(_))).count();
    assert_eq!(num_removed, 1usize);
    let num_added = changes.iter().filter(|sched_change| matches!(sched_change, SchedChange::Add(_, _))).count();
    assert!(num_added > 0usize);

    for change  in changes.iter() {
        if let SchedChange::Add(time, assignment) = change {
            let mut new_schedule = reservation_schedule.clone();
            new_schedule.schedule.insert(*time, *assignment);
            assert!(new_schedule.check_consistency());
            assert!(request.satisfies_request(time, assignment.2));
        }
    }

}

fn test_assign_unassign_states() {
    let resources = vec!["station1".to_string(), "station2".to_string()];
    let mut reservation_system = SyncReservationSystem::create_new_with_resources(&resources);

    let alternative1 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "station1".to_string(),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(NoCost{}),
    };

    let simple_request = vec![alternative1.clone()];
}

#[cfg(test)]
#[test]
fn test_unassign() {
    let time = Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap();
    let state = ReservationState { 
        unassigned: HashSet::from_iter(vec!(1)), 
        assigned: HashMap::from_iter([(0usize, "station1".to_string())]), 
        assignments: HashMap::from_iter([
            ("station2".to_string(), ReservationSchedule::new()), 
            ("station1".to_string(), ReservationSchedule { 
                schedule: BTreeMap::from_iter([
                    (time, Assignment(0, 0, Some(Duration::minutes(30)))
                )])
            })
        ])
        }; 
    
    let unassigned = state.create_unassign_state(time, &"station1".to_string());
    
    assert_eq!(unassigned.unassigned.len(), 2);
    assert_eq!(unassigned.assigned.len(), 0);

    assert_eq!(unassigned.assignments[&"station2".to_string()].schedule.len(), 0);
    assert_eq!(unassigned.assignments[&"station1".to_string()].schedule.len(), 0);

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
        cost_function: Arc::new(NoCost{}),
    };

    let simple_request = vec![alternative1.clone()];

    assert!(reservation_system.current_state.assignments["station1"].schedule.len() == 0);
    reservation_system.request_reservation(simple_request);
    assert!(reservation_system.current_state.assignments["station1"].schedule.len() == 1);


    let alternative2 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "station2".to_string(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(NoCost{}),
    };

    let multiple_alternatives = vec![alternative1, alternative2];
    reservation_system.request_reservation(multiple_alternatives);
    assert!(reservation_system.current_state.assignments["station1"].check_consistency());
    assert!(reservation_system.current_state.assignments["station2"].check_consistency());
}

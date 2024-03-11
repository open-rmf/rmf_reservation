#![feature(btree_cursors)]
#![feature(hasher_prefixfree_extras)]
#![feature(test)]
extern crate test;

use ::std::sync::{Arc, Mutex};
use std::collections::BinaryHeap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::thread::JoinHandle;

use chrono::{Date, DateTime, Duration, TimeZone, Utc};
use itertools::Itertools;

use std::collections::{
    btree_map::BTreeMap, hash_map::HashMap, hash_set::HashSet, vec_deque::VecDeque,
};
use std::hash::{Hash, Hasher};

use serde;
use serde_derive::{Deserialize, Serialize};

mod utils;
pub mod wait_points;

pub mod algorithms;

pub mod cost_function;

pub mod scenario_generation;

pub mod discretization;

pub mod database;

/// Constraints on start time
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct StartTimeRange {
    /// This is the earliest start time. If None, means there is no earliest time.
    pub earliest_start: Option<DateTime<Utc>>,
    /// This is the latest start time. If None, means there is no latest time.
    pub latest_start: Option<DateTime<Utc>>,
}

impl StartTimeRange {
    /// Create a time range with no specification
    pub fn no_specifics() -> Self {
        Self {
            earliest_start: None,
            latest_start: None,
        }
    }

     /// Create a time range which starts exactly at some given time.
    pub fn exactly_at(time: &DateTime<Utc>) -> Self {
        Self {
            earliest_start: Some(time.clone()),
            latest_start: Some(time.clone()),
        }
    }
}

mod duration_serialization {
    use chrono::Duration;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(dur: &Option<Duration>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match dur {
            Some(dur) => ser.serialize_i64(dur.num_milliseconds()),
            None => ser.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserialize: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<i64> = Option::deserialize(deserialize)?;
        if let Some(s) = s {
            return Ok(Some(Duration::milliseconds(s)));
        }
        Ok(None)
    }
}
/// Reservation request parameters
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReservationParameters {
    /// Resource you want to reserve
    pub resource_name: String,
    /// Duration of the reservation. If the duration is none, it is assumed this is an indefinite request.
    /// I.E this reservation is permananent.
    #[serde(default)]
    #[serde(with = "duration_serialization")]
    pub duration: Option<Duration>,
    /// Start time constraints.
    pub start_time: StartTimeRange,
}

/// Implements a simple cost function model
pub trait CostFunction {
    /// Givern some parameters and the start time return the cost of a reservation starting at the instant
    fn cost(&self, parameters: &ReservationParameters, instant: &DateTime<Utc>) -> f64;
}

/// Reservation Request represents one possible alternative reservation
#[derive(Clone)]
pub struct ReservationRequest {
    /// The parameters
    pub parameters: ReservationParameters,
    /// Cost function to use
    pub cost_function: Arc<dyn CostFunction + Send + Sync>,
}
impl fmt::Debug for ReservationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReservationRequest")
            .field("parameters", &self.parameters)
            .finish()
    }
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

    /// Returns if it is possible for this request to be scheduled after the request in alternative
    pub fn can_be_scheduled_after(&self, alternative: &ReservationParameters) -> bool {
        if let Some(alt) = alternative.start_time.latest_start {
            if let Some(dur) = self.parameters.duration {
                let Some(earliest_start) = self.parameters.start_time.earliest_start else {
                    return true;
                };
                let earliest_end_time = earliest_start + dur;
                earliest_end_time < alt
            } else {
                false
            }
        } else {
            if let Some(_) = self.parameters.duration {
                true
            } else {
                false
            }
        }
    }

    /// Check if the current request has to always be after the previous request.
    pub fn is_always_after(&self, alternative: &ReservationParameters) -> bool {
        if let Some(earliest) = self.parameters.start_time.earliest_start {
            if let Some(duration_alt) = alternative.duration {
                if let Some(alt_latest_start) = alternative.start_time.latest_start {
                    alt_latest_start < earliest
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Hash, Eq)]
struct Assignment(usize, usize, Option<Duration>);

/// Represents a single resource's schedule.
#[derive(Clone, Debug, PartialEq, Hash, Eq)]
struct ReservationSchedule {
    schedule: BTreeMap<DateTime<Utc>, Assignment>,
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

    pub fn insert(
        &mut self,
        index: (usize, usize),
        duration: Option<Duration>,
        start: DateTime<Utc>,
    ) {
        self.schedule
            .insert(start, Assignment(index.0, index.1, duration));
    }

    /// Collect garbage up to time.
    pub fn garbage_collect(&mut self, time: DateTime<Utc>) {
        self.schedule = self.schedule.split_off(&time);
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
        claimed_requests: &HashSet<usize>,
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
            if claimed_requests.contains(&assignment.0) {
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
    pub fn garbage_collect(&mut self, time: DateTime<Utc>) {
        for (_, schedule) in &mut self.assignments {
            schedule.garbage_collect(time);
        }
    }

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
        new_self
            .assigned
            .insert(assignment.0, (resource.clone(), time));
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
        claimed_requests: &HashSet<usize>,
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

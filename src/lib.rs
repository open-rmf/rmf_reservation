use ::std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{btree_map::BTreeMap, hash_map::HashMap, hash_set::HashSet};
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

#[derive(Copy, Clone, Debug, PartialEq, Hash)]
struct Assignment(usize, usize, Option<Duration>);

/// Represents a single resource's schedule.
#[derive(Clone, Debug, PartialEq, Hash)]
struct ReservationSchedule {
    schedule: BTreeMap<DateTime<Utc>, Assignment>,
}

enum NextInstant {
    Beginning,
    NextInstant(DateTime<Utc>),
    NoMoreAllowed,
}

impl ReservationSchedule {
    /// Checks consistency
    fn check_consistency(&self) -> bool {
        // Use unix epoch
        let mut next_min_instant = NextInstant::Beginning;
        for (instant, assignment) in &self.schedule {
            match next_min_instant {
                NextInstant::NoMoreAllowed => {
                    return false;
                }
                _ => {
                    if let NextInstant::NextInstant(time) = next_min_instant {
                        if *instant <= time {
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
}

#[derive(Clone, Debug, PartialEq)]
struct ReservationState {
    unassigned: HashSet<usize>,
    assigned: HashSet<usize>,
    assignments: ReservationSchedule,
}

impl Hash for ReservationState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for f in &self.unassigned {
            f.hash(state);
        }

        for f in &self.assigned {
            f.hash(state);
        }

        self.assignments.hash(state);
    }
}

impl ReservationState {
    fn create_unassign_state(self, time: DateTime<Utc>) -> Self {
        let mut new_self = self; // TODO(arjo): Implement views via traits.
        if let Some((_, assignment)) = new_self.assignments.schedule.remove_entry(&time) {
            new_self.assigned.remove(&assignment.0);
            new_self.unassigned.insert(assignment.0);
        }
        new_self
    }
}

pub struct SyncReservationSystem {
    reservation_queue: Vec<Vec<ReservationRequest>>,
    current_state: HashMap<String, ReservationState>,
    cummulative_cost: f64,
}

impl SyncReservationSystem {
    pub fn new() -> Self {
        Self {
            reservation_queue: vec![],
            current_state: HashMap::new(),
            cummulative_cost: 0f64,
        }
    }

    pub fn request_reservation(&mut self, reservations: Vec<ReservationRequest>) {
        self.reservation_queue.push(reservations);
        // Look inside
        for _alternative in &self.reservation_queue[self.reservation_queue.len() - 1] {
            //self.attempt_insert(alternative);
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

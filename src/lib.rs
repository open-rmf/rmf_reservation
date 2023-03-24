use ::std::sync::Arc;
use chrono::prelude::*;
use chrono::{DateTime, Duration, Utc};
use std::collections::{btree_map::BTreeMap, hash_map::HashMap, hash_set::HashSet};
use std::ops::Bound::{Excluded, Included};

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
        intervals: Duration,
        earliest_start: Option<DateTime<Utc>>,
        latest_start: Option<DateTime<Utc>>,
    ) -> Vec<PotentialAssignment> {
        vec![]
    }
}

#[derive(Copy, Clone, Debug)]
struct Assignment(usize, usize, Option<Duration>);

#[derive(Clone, Debug)]
struct ReservationState {
    unassigned: HashSet<usize>,
    assigned: HashSet<usize>,
    assignments: BTreeMap<DateTime<Utc>, Assignment>,
}

impl ReservationState {
    fn create_unassign_state(self, time: DateTime<Utc>) -> Self {
        let mut new_self = self.clone(); // TODO(arjo): Implement views via traits.
        if let Some((_, assignment)) = new_self.assignments.remove_entry(&time) {
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
        for alternative in &self.reservation_queue[self.reservation_queue.len() - 1] {
            //self.attempt_insert(alternative);
        }
    }

    /// Returns a list of assignments that may have a potential conflict with the relevant ReservationRequest
    /// TODO: Change to return range
    fn check_potential_conflict(
        &self,
        reservation: &ReservationRequest,
    ) -> Vec<(DateTime<Utc>, Assignment)> {
        let earliest_time = if let Some(earliest) = reservation.parameters.start_time.earliest_start
        {
            Some(earliest)
        } else {
            if let Some(res) = self
                .current_state
                .get(&reservation.parameters.resource_name)
            {
                if let Some((k, _v)) = res.assignments.first_key_value() {
                    Some(*k)
                } else {
                    None
                }
            } else {
                None
            }
        };

        let latest_time = reservation.parameters.start_time.latest_start;
        if let Some(schedule) = self
            .current_state
            .get(&reservation.parameters.resource_name)
        {
            // Identify potential conflicting range.
            let range = if let Some(latest_time) = latest_time {
                if let Some(earliest_time) = earliest_time {
                    if let Some(duration) = reservation.parameters.duration {
                        // If a reservation has a lower bound and upper bound on a starting time
                        // and it has a fixed minimum duration then the range wich conflicts is as follows.
                        // Note: We will also need to find the reservation just before the earliest time
                        schedule
                            .assignments
                            .range(earliest_time..latest_time + duration)
                    } else {
                        // If it only has no duration but has a lower bound, then conflicts will take place
                        // after the earliest time.
                        schedule.assignments.range(earliest_time..)
                    }
                } else {
                    if let Some(duration) = reservation.parameters.duration {
                        // If it has a duration and only a latest time then conflicts will take place before
                        // or up to the latest time
                        schedule.assignments.range(..latest_time + duration)
                    } else {
                        // If it has a duration and no earlst time.
                        schedule.assignments.range(latest_time..)
                    }
                }
            } else {
                if let Some(earliest_time) = earliest_time {
                    schedule.assignments.range(earliest_time..)
                } else {
                    schedule.assignments.range(..)
                }
            };

            // Identify conflicts.
            let mut conflicts = vec![];
            // First check the earliest time we can assign
            if let Some(earliest_time) = earliest_time {
                if let Some((instant, assignment)) =
                    schedule.assignments.range(..earliest_time).next_back()
                {
                    if let Some(duration) = assignment.2 {
                        if let Some(latest_time) = reservation.parameters.start_time.latest_start {
                            if latest_time > *instant + duration {
                                conflicts.push((*instant, *assignment))
                            }
                        }
                    } else {
                        // Indefinite reservation. The previous reservation will conflict.
                        conflicts.push((*instant, *assignment));
                    }
                }
            }

            for (instant, assignment) in range {
                conflicts.push((*instant, *assignment));
            }

            return conflicts;
        }
        vec![]
    }
}

struct NoCost {}

impl CostFunction for NoCost {
    fn cost(&self, parameters: &ReservationParameters, instant: &DateTime<Utc>) -> f64 {
        0f64
    }
}

#[cfg(test)]
#[test]
fn test_conflict_checker() {
    let mut res_sys = SyncReservationSystem::new();
    let cost_func = Arc::new(NoCost {});

    // Check for an indefinite reservation with no specification on
    let mut indefinite_no_constraints = ReservationRequest {
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

    let res = res_sys.check_potential_conflict(&indefinite_no_constraints);
    assert_eq!(res.len(), 0);

    res_sys.reservation_queue.push(vec![ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource2".to_string(),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: None,
            },
            duration: None,
        },
        cost_function: cost_func.clone(),
    }]);

    let mut assign_first_item = HashSet::new();
    assign_first_item.insert(0usize);
    let mut schedule = BTreeMap::new();
    schedule.insert(
        Utc.with_ymd_and_hms(2023, 7, 8, 9, 10, 11).unwrap(),
        Assignment(0usize, 0usize, None),
    );
    res_sys.current_state.insert(
        "resource2".to_string(),
        ReservationState {
            unassigned: HashSet::new(),
            assigned: assign_first_item,
            assignments: schedule,
        },
    );

    let res = res_sys.check_potential_conflict(&indefinite_no_constraints);
    assert_eq!(res.len(), 0);

    let mut indefinite_no_constraints = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: "resource2".to_string(),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: None,
            },
            duration: None,
        },
        cost_function: cost_func.clone(),
    };
}

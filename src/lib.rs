//! This is a library that provides a solver for constrained resource scheduling for multi-robot applications. More specifically,
//! we provide a very simple formulation for resource optimization and scheduling. A robot may request the use of a
//! resource like a charger for a fixed duration of time within a given time range. The system will then assign the robot
//! to said resource.
//!
//! At 10,000ft the library solves the following problem:
//! > Suppose you have n robots declaring that “I'd like to use one of (resource 1, resource 2, resource 3) for (d1, d2, d3) minutes starting at a certain time. Each alternative has some cost c(t).”
//!
//! To specify such a problem take a look at [ReservationRequestAlternative] which provides a wrapper around one such alternative.

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
pub struct ReservationRequestAlternative {
    /// The parameters
    pub parameters: ReservationParameters,
    /// Cost function to use
    pub cost_function: Arc<dyn CostFunction + Send + Sync>,
}
impl fmt::Debug for ReservationRequestAlternative {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReservationRequest")
            .field("parameters", &self.parameters)
            .finish()
    }
}

impl ReservationRequestAlternative {
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
        reservation: &ReservationRequestAlternative,
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

#[cfg(test)]
#[test]
fn test_satisfies_request() {
    use crate::cost_function::static_cost::StaticCost;

    let req = ReservationRequestAlternative {
        parameters: ReservationParameters {
            resource_name: "test".to_string(),
            duration: Some(Duration::minutes(30)),
            start_time: StartTimeRange {
                earliest_start: None,
                latest_start: None,
            },
        },
        cost_function: Arc::new(StaticCost::new(0.0)),
    };

    let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap();
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(30))));
    assert!(req.satisfies_request(&start_time, Some(Duration::minutes(20))) == false);
    assert!(req.satisfies_request(&start_time, None) == false);

    let req = ReservationRequestAlternative {
        parameters: ReservationParameters {
            resource_name: "test".to_string(),
            duration: Some(Duration::minutes(30)),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 8, 10, 11).unwrap()),
            },
        },
        cost_function: Arc::new(StaticCost::new(0.0)),
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
    use crate::cost_function::static_cost::StaticCost;

    let cost_func = Arc::new(StaticCost::new(0.0));

    // Check for an indefinite reservation with no specification on
    let indefinite_no_constraints = ReservationRequestAlternative {
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

    let indefinite_with_constraints = ReservationRequestAlternative {
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
    let definite_request_starting_with_specified_start_time = ReservationRequestAlternative {
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

    let definite_request_with_no_earliest = ReservationRequestAlternative {
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

    let definite_request_with_no_latest = ReservationRequestAlternative {
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

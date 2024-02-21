use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde_derive::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TicketResponse {
    pub selected_index: usize,
    uuid: String,
}

impl TicketResponse {
    fn new(selected_index: usize) -> Self {
        Self {
            selected_index,
            uuid: Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WaitPointInfo {
    time: Option<DateTime<Utc>>,
    uuid: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WaitPointRequest {
    pub wait_point: String,
    pub time: DateTime<Utc>,
}

/// Waitpoints are safe holding points. There should be the same or
/// more wait points than robots on a map.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct WaitPointSystem {
    wait_points: HashMap<String, VecDeque<WaitPointInfo>>,
    uuid_map: HashMap<String, String>,
}

impl WaitPointSystem {
    /// Request a waitpoint
    pub fn request_waitpoint(
        &mut self,
        wait_point_list: &Vec<WaitPointRequest>,
    ) -> Result<TicketResponse, String> {
        let mut res = 0usize;
        for wait_point in wait_point_list {
            let is_free = self
                .wait_points
                .get(&wait_point.wait_point)
                .and_then(|x| x.back())
                .map_or_else(
                    || true,
                    |time| {
                        if let Some(time) = time.time {
                            if time < wait_point.time {
                                return true;
                            }
                        }
                        false
                    },
                );

            if is_free {
                let ticket_response = TicketResponse::new(res);
                let uuid = ticket_response.uuid.clone();
                self.uuid_map
                    .insert(uuid.clone(), wait_point.wait_point.clone());
                if let Some(mut index) = self.wait_points.get_mut(&wait_point.wait_point) {
                    index.push_back(WaitPointInfo {
                        time: None,
                        uuid: uuid,
                    });
                } else {
                    let mut vec = VecDeque::new();
                    vec.push_back(WaitPointInfo {
                        time: None,
                        uuid: uuid,
                    });
                    self.wait_points.insert(wait_point.wait_point.clone(), vec);
                }
                return Ok(ticket_response);
            }

            res += 1usize;
        }

        Err("No solution found".to_string())
    }

    // TODO(arjoc) make
    pub fn block_waitpoints(&mut self, wait_points: &Vec<String>) -> Result<(), &str> {
        for wait_point in wait_points {
            if let Some(mut index) = self.wait_points.get_mut(wait_point) {
                if index.len() > 0 {
                    return Err("");
                }
                index.push_back(WaitPointInfo {
                    time: None,
                    uuid: Uuid::new_v4().to_string(),
                });
            } else {
                self.wait_points.insert(
                    wait_point.clone(),
                    VecDeque::from_iter(
                        [WaitPointInfo {
                            time: None,
                            uuid: Uuid::new_v4().to_string(),
                        }]
                        .into_iter(),
                    ),
                );
            }
        }
        Ok(())
    }

    pub fn release_waitpoint_at_time(
        &mut self,
        wait_point: &String,
        time: &DateTime<Utc>,
    ) -> Result<(), &str> {
        if let Some(mut wait_point) = self.wait_points.get_mut(wait_point) {
            if let Some(mut waitpoint) = wait_point.back_mut() {
                waitpoint.time = Some(*time);
                Ok(())
            } else {
                Err("Wait point was empty")
            }
        } else {
            Err("No waitpoint with given name found.")
        }
    }

    pub fn flush_until(&mut self, time: &DateTime<Utc>) {
        for (_, queue) in self.wait_points.iter_mut() {
            while let Some(qitem) = queue.front() {
                if qitem.time == None {
                    break;
                }
                if let Some(curr_time) = qitem.time {
                    if curr_time >= *time {
                        break;
                    }
                }

                queue.pop_front();
            }
        }
    }
}

#[cfg(test)]
fn test_time(time: i64) -> DateTime<Utc> {
    use chrono::{Duration, TimeZone};

    let base = Utc.with_ymd_and_hms(2023, 7, 8, 0, 0, 0).unwrap();

    let dur = Duration::seconds(time);

    base + dur
}

#[cfg(test)]
#[test]
fn test_waitpoint() {
    let mut wait_point_system = WaitPointSystem::default();

    let wp1 = "wp1".to_string();
    let wp2 = "wp2".to_string();
    let time = test_time(10);
    let wait_points = vec![
        WaitPointRequest {
            wait_point: wp1.clone(),
            time: time,
        },
        WaitPointRequest {
            wait_point: wp2.clone(),
            time: time,
        },
    ];
    let result = wait_point_system.request_waitpoint(&wait_points);
    // Could allocate a waitpoint
    assert!(result.is_ok());
    // Order by requests sent so waypoint1 should be selected.
    assert_eq!(result.clone().unwrap().selected_index, 0usize);

    let claim_time = test_time(5);
    let result1 = wait_point_system.release_waitpoint_at_time(&wp1, &claim_time);
    assert!(result1.is_ok());

    // Attempt invalid request
    let result1 = wait_point_system.release_waitpoint_at_time(&wp2, &claim_time);
    assert!(result1.is_err());

    // Try again, we should get the same spot as we arrive early.
    let result_wp1 = wait_point_system.request_waitpoint(&wait_points);
    // Could allocate a waitpoint
    assert!(result_wp1.is_ok());
    // Order by requests sent so waypoint1 should be selected.
    assert_eq!(result_wp1.clone().unwrap().selected_index, 0usize);

    // We should get the same request.
    let result = wait_point_system.request_waitpoint(&wait_points);
    // Could allocate a waitpoint
    assert!(result.is_ok());
    // Waypoint 0 has not yet been claimed. Waypoint 1 has to be used.
    assert_eq!(result.clone().unwrap().selected_index, 1usize);
}

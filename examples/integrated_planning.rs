use chrono::{Date, DateTime, Duration, Utc};
use mapf::{
    motion::{
        se2::{DifferentialDriveLineFollow, LinearTrajectorySE2, Point, WaypointSE2},
        trajectory, CcbsEnvironment, CircularProfile, DynamicCircularObstacle, DynamicEnvironment,
        Trajectory, TravelEffortCost,
    },
    prelude::{AStarConnect, SharedGraph, SimpleGraph},
    premade::SippSE2,
    templates::InformedSearch,
    Planner,
};
use rmf_reservations::{
    AsyncReservationSystem, ClaimResult, CostFunction, RequestError, ReservationParameters,
    ReservationRequest, ReservationVoucher, StartTimeRange,
};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::{collections::HashMap, hash::Hash, sync::Arc, thread::JoinHandle};

struct NoCost {}

impl CostFunction for NoCost {
    fn cost(&self, _parameters: &ReservationParameters, _instant: &DateTime<Utc>) -> f64 {
        0f64
    }
}

pub struct MultiRobotScheduler {
    res_sys: AsyncReservationSystem,
    name_to_idx: HashMap<String, usize>,
    chargers: Vec<String>,
    charger_pos: Vec<(f64, f64)>,
    res_sys_thread: JoinHandle<()>,
}

impl MultiRobotScheduler {
    fn init(num: usize) -> Self {
        // TODO(arjo): generate scale
        let mut chargers: Vec<String> = vec![];
        let mut charger_pos = vec![];
        let mut name_to_idx = HashMap::new();

        for i in 0..num {
            let name = format!("charger {:?}", i);
            chargers.push(name.clone());
            charger_pos.push((5.0, num as f64));
            name_to_idx.insert(name, i);
        }
        let res_sys = AsyncReservationSystem::new(&chargers);
        let res_sys_thread = res_sys.spin_in_bg();
        Self {
            res_sys,
            chargers,
            charger_pos,
            name_to_idx,
            res_sys_thread,
        }
    }

    fn request_charge_at(
        &mut self,
        time: DateTime<chrono::Utc>,
        battery_out: DateTime<chrono::Utc>,
        duration: Duration,
    ) -> Result<ReservationVoucher, RequestError> {
        let cost_func = Arc::new(NoCost {});
        let chargers = self.chargers.iter().map(|charger| ReservationRequest {
            parameters: ReservationParameters {
                resource_name: charger.clone(),
                duration: Some(duration.clone()),
                start_time: StartTimeRange {
                    earliest_start: Some(time),
                    latest_start: Some(battery_out),
                },
            },
            cost_function: cost_func.clone(),
        });
        self.res_sys.request_reservation(chargers.collect())
    }

    fn garbage_collect(&mut self, time: DateTime<Utc>) {
        self.res_sys.garbage_collect(time);
    }

    pub fn claim(&self, voucher: ReservationVoucher) -> ClaimResult {
        self.res_sys.claim(voucher)
    }

    pub fn get_positon(&self, str: String) -> Option<(f64, f64)> {
        let Some(idx) = self.name_to_idx.get(&str) else {
            return None;
        };

        Some(self.charger_pos[*idx])
    }
}

fn diff(pt1: (f64, f64), pt2: (f64, f64)) -> (f64, f64) {
    (pt1.0 - pt2.0, pt1.1 - pt2.1)
}

fn normalize(pt1: (f64, f64)) -> (f64, f64) {
    let len = (pt1.0 * pt1.0 + pt1.1 * pt1.1).sqrt();
    scale(pt1, 1.0 / len)
}

fn scale(pt1: (f64, f64), scale_factor: f64) -> (f64, f64) {
    (pt1.0 * scale_factor, pt1.1 * scale_factor)
}

const SPEED: f64 = 1.0;

enum State {
    MOVE_TOWARDS_CHARGER((f64, f64)),
    MOVE_AWAY_CHARGER,
    CHARGING(DateTime<Utc>),
    PARKED,
}
struct Robot {
    pos: (f64, f64),
    vel: (f64, f64),
    starting_spot: (f64, f64),
    state: State,
    last_parked: DateTime<Utc>,
    voucher: Option<ReservationVoucher>,
    id: usize,
}

impl Robot {
    fn init(starting_pos: &(f64, f64), id: usize) -> Self {
        Self {
            pos: starting_pos.clone(),
            vel: (0.0, 0.0),
            starting_spot: starting_pos.clone(),
            state: State::PARKED,
            last_parked: DateTime::<Utc>::MIN_UTC,
            voucher: None,
            id,
        }
    }
    fn step(&mut self, dt: f64) {
        self.pos.0 += self.vel.0 * dt;
        self.pos.1 += self.vel.1 * dt;
    }

    async fn take_decision(
        &mut self,
        robot_sched: &mut MultiRobotScheduler,
        current_time: DateTime<Utc>,
    ) {
        let CHARGE_DURATION: Duration = Duration::seconds(500);
        let WORK_DURATION: Duration = Duration::seconds(500);
        let MAX_WORK_DURATION: Duration = Duration::seconds(200);
        match self.state {
            State::MOVE_TOWARDS_CHARGER(point) => {
                let dx = point.0 - self.pos.0;
                let dy = point.1 - self.pos.1;

                if dx * dx + dy * dy < 0.1 {
                    println!(
                        "At time {:?} robot {:?} reaches charger at {:?}",
                        current_time, self.id, point
                    );
                    self.state = State::CHARGING(current_time + CHARGE_DURATION);
                    self.vel = (0.0, 0.0);
                }
            }
            State::CHARGING(time_to_move) => {
                if current_time >= time_to_move {
                    println!(
                        "At time {:?} robot {:?} moves away from charger",
                        current_time, self.id
                    );
                    self.state = State::MOVE_AWAY_CHARGER;
                    self.vel = scale(diff(self.starting_spot, self.pos), SPEED);
                }
            }
            State::MOVE_AWAY_CHARGER => {
                let dx = self.starting_spot.0 - self.pos.0;
                let dy = self.starting_spot.1 - self.pos.1;

                if dx * dx + dy * dy < 0.1 {
                    self.state = State::PARKED;
                    self.vel = (0.0, 0.0);
                    self.last_parked = current_time;
                    println!("At time {:?} robot {:?} is parked", current_time, self.id);

                    let Ok(voucher) = robot_sched.request_charge_at(
                        self.last_parked + WORK_DURATION,
                        self.last_parked + MAX_WORK_DURATION,
                        CHARGE_DURATION,
                    ) else {
                        panic!("Failed to get voucher");
                    };
                    self.voucher = Some(voucher);
                }
            }
            State::PARKED => {
                if self.voucher.is_none() {
                    let Ok(voucher) = robot_sched.request_charge_at(
                        self.last_parked + WORK_DURATION,
                        self.last_parked + MAX_WORK_DURATION,
                        CHARGE_DURATION,
                    ) else {
                        panic!("Failed to get voucher");
                    };
                    println!("Robot {:?} requested reservation.", self.id);
                    self.voucher = Some(voucher);
                    return;
                }
                if current_time > self.last_parked + WORK_DURATION {
                    println!("Robot {:?} move", self.id);
                    let Some(voucher) = self.voucher.clone() else {
                        panic!("No voucher")
                    };
                    let Ok(claim) = robot_sched.claim(voucher).await else {
                        panic!("Failed to get next destination")
                    };
                    println!("Robot {:?} moving from parked spot to {:?}", self.id, claim);
                    let next_pos = robot_sched.get_positon(claim).unwrap();
                    self.state = State::MOVE_TOWARDS_CHARGER(next_pos);
                    self.vel = scale(diff(next_pos, self.pos), SPEED);
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let num_robots = 4;
    let mut scheduler = MultiRobotScheduler::init(num_robots);

    let mut time_now = DateTime::<Utc>::MIN_UTC;

    let mut robots = {
        let mut robot_list = vec![];
        for i in 0..num_robots {
            robot_list.push(Robot::init(&(0.0, i as f64), i));
        }
        robot_list
    };

    for _i in 0..10000 {
        //scheduler.garbage_collect(time_now);
        time_now += Duration::seconds(1);

        for robot in &mut robots {
            robot.step(0.1);
            robot.take_decision(&mut scheduler, time_now).await;
        }
    }
    Ok(())
}

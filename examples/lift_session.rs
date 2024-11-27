use std::{collections::HashMap, io::Write, sync::{atomic::AtomicBool, Arc}, time::SystemTime};

use chrono::{TimeZone, Utc};
use rmf_reservations::{
    algorithms::sat_flexible_time_model::{Problem, SATFlexibleTimeModel}, cost_function::static_cost, database::ClockSource, ReservationParameters, ReservationRequestAlternative, StartTimeRange
};

use rand::Rng;


#[derive(Default, Clone)]
struct FakeClock;

impl ClockSource for FakeClock {
    fn now(&self) -> chrono::DateTime<chrono::prelude::Utc> {
        let time = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap();
        return time;
    }
}

struct Robot {
    time_from_start_to_lift_travel: Vec<chrono::Duration>,
    time_from_lift_to_destination: Vec<chrono::Duration>,
    start_floor: usize,
    end_floor: usize,
}

struct Lift;

struct LiftAssigner {
    robots: Vec<Robot>,
    lifts: Vec<Lift>,
    lift_time_per_floor: chrono::Duration,
    start_time: chrono::DateTime<Utc>,
}

impl LiftAssigner {
    fn get_problem(&self) -> Problem {
        let mut problem = Problem::default();
        //let earliest_start = 
        let mut request_id_to_lift: HashMap<usize, Vec<(usize, usize, (usize, usize))>> = HashMap::new();
        for robot in &self.robots {
            let mut lift_alternatives = vec![];
            let mut start_and_dest_floors = vec![];
            for lift_id in 0..self.lifts.len() {
                let Some(time_to_lift) = robot.time_from_start_to_lift_travel.get(lift_id) else {
                    continue;
                };
                //let time_from_lift_to_dest = robot.time_from_lift_to_destination[lift_id];
                let diff = robot.start_floor.abs_diff(robot.end_floor);
                let time_in_lift = (0..diff).fold(chrono::Duration::new(0, 0).unwrap(), |a, _| {
                    a + self.lift_time_per_floor
                });
                let req = ReservationRequestAlternative {
                    cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
                    parameters: ReservationParameters {
                        resource_name: format!("lift_{}", lift_id),
                        start_time: StartTimeRange {
                            earliest_start: Some(self.start_time + *time_to_lift),
                            latest_start: None
                        },
                        duration: Some(time_in_lift)
                    }
                };
                lift_alternatives.push(req);
                start_and_dest_floors.push((lift_id, robot.start_floor, robot.end_floor));
            }

            let req_id = problem.request_one_of(lift_alternatives);

            // Calculate transition costs. For that let us group by lift_ids first
            for i in 0..start_and_dest_floors.len() {
                let (lift_id, start_floor, dest_floor) = start_and_dest_floors[i];
                if let Some(p) = request_id_to_lift.get_mut(&lift_id) {
                    p.push((start_floor, dest_floor, (req_id, i)));
                }
                else {
                    request_id_to_lift.insert(lift_id, vec![(start_floor, dest_floor, (req_id, i))]);
                }
            }

            // Now calculate transition costs
            for (_lift_id, tasks) in &request_id_to_lift {
                for from in 0..tasks.len() {
                    for to in 0..tasks.len() {
                        if from == to {
                            continue;
                        }
                        let prev_dest = tasks[from].1;
                        let prev_req_id = tasks[from].2;

                        let next_pickup = tasks[to].0;
                        let next_req_id = tasks[to].2;

                        let diff = prev_dest.abs_diff(next_pickup);
                        let transition_time = (0..diff).fold(chrono::Duration::new(0, 0).unwrap(), |a, _| {
                                 a + self.lift_time_per_floor
                            });

                        // We can encode the minimum transition time to be dependent on the next. 
                        problem.require_minimum_gap(&prev_req_id, &next_req_id, transition_time);
                    }
                }
            }
        }

        problem
    }
}

fn main() {

    let num_robots = 10;
    let max_floors = 30;
    let num_lifts = 2;

    let max_time_to = 500;

    let mut num = rand::thread_rng();
    
    let robots: Vec<_> = (0..num_robots).map(|_|{
        Robot {
            time_from_start_to_lift_travel: (0..num_lifts).map(|_| chrono::Duration::new(num.gen_range(0..max_time_to), 0).unwrap()).collect(),
            time_from_lift_to_destination: (0..num_lifts).map(|_| chrono::Duration::new(num.gen_range(0..max_time_to), 0).unwrap()).collect(),
            start_floor: num.gen_range(1..max_floors),
            end_floor: num.gen_range(1..max_floors),
        }
    }).collect();

    let lifts: Vec<_> = (0..num_lifts).map(|_|{Lift}).collect();
    let my_clock = FakeClock::default();
    let lift_assigner = LiftAssigner {
        robots,
        lifts,
        lift_time_per_floor: chrono::Duration::new(60, 0).unwrap(),
        start_time: my_clock.now()
    };

    let problem = lift_assigner.get_problem();
    let (sender, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = SATFlexibleTimeModel {
        clock_source: my_clock
    };

    
    
    let child = std::thread::spawn(move || {
        let timer = SystemTime::now();
        s.time_suboptimal_search_solver(&problem, sender, stop, 2);
        println!("Optimality in: {:?}", timer.elapsed());
    });
    let timer = SystemTime::now();
    let mut file = std::fs::File::create("perf.txt").unwrap();
    for c in rx.iter() {
        let res = format!("Sub-optimal solution found in: {:?}\n", timer.elapsed());
        file.write(&res.as_bytes());
    }
    
}

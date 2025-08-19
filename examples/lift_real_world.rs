use std::{
    collections::{HashMap, HashSet},
    hash::RandomState,
    io::Write,
    sync::{atomic::AtomicBool, Arc},
    time::SystemTime,
};

use chrono::{DateTime, TimeZone, Utc};
use rmf_reservations::{
    algorithms::{
        sat_flexible_time_model::{Assignment, Problem, SATFlexibleTimeModel},
        sat_teg::TEGSolver,
    },
    cost_function::static_cost,
    database::ClockSource,
    ReservationParameters, ReservationRequestAlternative, StartTimeRange,
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
#[derive(Clone, Debug)]
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
        let mut request_id_to_lift: HashMap<usize, Vec<(usize, usize, (usize, usize))>> =
            HashMap::new();
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
                            latest_start: None,
                        },
                        duration: Some(time_in_lift),
                    },
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
                } else {
                    request_id_to_lift
                        .insert(lift_id, vec![(start_floor, dest_floor, (req_id, i))]);
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
                        let transition_time = (0..diff)
                            .fold(chrono::Duration::new(0, 0).unwrap(), |a, _| {
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

    // This uses the greedy policy to get the makespan.
    fn greedy_policy(&self) -> chrono::Duration {
        let mut nearest_lift_queue = HashMap::new();
        for (id, robot) in self.robots.iter().enumerate() {
            nearest_lift_queue.insert(
                id,
                robot
                    .time_from_start_to_lift_travel
                    .iter()
                    .enumerate()
                    .map(|(a, b)| (*b, a))
                    .min()
                    .unwrap()
                    .1,
            );
        }

        let mut lift_queues: HashMap<usize, Vec<usize>> = HashMap::new();
        for (robot_id, lift_id) in nearest_lift_queue.iter() {
            if let Some(lift_queue) = lift_queues.get_mut(lift_id) {
                lift_queue.push(*robot_id);
            } else {
                lift_queues.insert(*lift_id, vec![*robot_id]);
            }
        }

        let mut max_make_span = chrono::Duration::zero();
        for (lift_id, _) in self.lifts.iter().enumerate() {
            let mut curr_time = chrono::Duration::zero();
            let mut current_floor = 0;
            if let Some(robots) = lift_queues.get(&lift_id) {
                let mut robots_2 = robots.clone();
                let mut robot_list = self.robots.clone();
                robots_2.sort_by(move |r1, r2| {
                    robot_list[*r1].time_from_start_to_lift_travel[lift_id]
                        .cmp(&robot_list[*r2].time_from_start_to_lift_travel[lift_id])
                });
                for robot in robots_2 {
                    let time_start = self.robots[robot].time_from_start_to_lift_travel[lift_id];
                    let x = (0..self.robots[robot].start_floor.abs_diff(current_floor))
                        .fold(chrono::Duration::zero(), |q, _| {
                            q + self.lift_time_per_floor
                        });
                    if curr_time + x < time_start {
                        curr_time = time_start;
                    } else {
                        curr_time += x;
                    }
                    let det_floor = self.robots[robot].end_floor;
                    curr_time += (0..self.robots[robot].start_floor.abs_diff(det_floor))
                        .fold(chrono::Duration::zero(), |q, _| {
                            q + self.lift_time_per_floor
                        });
                    current_floor = det_floor;
                }
                max_make_span = max_make_span.max(curr_time);
                // Nearest floor heuristic ignoring constraints
                /*let mut current_lift_pos = 1;
                let mut robots_to_pick_up: HashSet<usize, RandomState> = HashSet::from_iter(robots.into_iter().map(|r| *r));

                while robots_to_pick_up.len() != 0 {
                    //let dist_to_lift = ;
                    let mut selected_floor = 100000usize;
                    let mut dest_floor = 10000000usize;
                    let mut selected_robot = 0usize;
                    for robot in robots_to_pick_up {
                        let start_floor = self.robots[robot].start_floor;
                        if selected_floor.abs_diff(start_floor) < selected_floor.abs_diff(current_lift_pos) {
                            selected_floor = start_floor;
                            dest_floor = self.robots[robot].end_floor;
                            selected_robot = robot;
                        }
                    }
                    current_lift_pos = dest_floor;
                    robots_to_pick_up.remove(&selected_robot);
                }*/
            }
        }
        max_make_span
    }
}

fn get_makespan(
    final_schedule: HashMap<String, Vec<Assignment>>,
    problem: Problem,
) -> Option<DateTime<Utc>> {
    final_schedule
        .iter()
        .filter(|(_resource, assignment)| assignment.len() != 0)
        .map(move |(_resource, assignment)| {
            let assignment = &assignment[assignment.len() - 1];
            assignment.start_time
                + problem.requests[assignment.id.0][assignment.id.1]
                    .parameters
                    .duration
                    .unwrap_or(chrono::Duration::new(0, 0).unwrap())
        })
        .max()
}

fn main() {
    let num_robots = 15;
    let max_floors = 30;
    let num_lifts = 6;

    let max_time_to = 300;

    let mut num = rand::thread_rng();

    let robots: Vec<_> = (0..num_robots)
        .map(|_| Robot {
            time_from_start_to_lift_travel: (0..num_lifts)
                .map(|_| chrono::Duration::new(num.gen_range(0..max_time_to), 0).unwrap())
                .collect(),
            time_from_lift_to_destination: (0..num_lifts)
                .map(|_| chrono::Duration::new(num.gen_range(0..max_time_to), 0).unwrap())
                .collect(),
            start_floor: num.gen_range(1..max_floors),
            end_floor: num.gen_range(1..max_floors),
        })
        .collect();

    let lifts: Vec<_> = (0..num_lifts).map(|_| Lift).collect();
    let my_clock = FakeClock::default();
    let lift_assigner = LiftAssigner {
        robots,
        lifts,
        lift_time_per_floor: chrono::Duration::new(15, 0).unwrap(),
        start_time: my_clock.now(),
    };

    let problem = lift_assigner.get_problem();
    let problem2 = problem.clone();
    let problem3 = problem.clone();
    let (sender, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = SATFlexibleTimeModel {
        clock_source: my_clock.clone(),
    };

    let child = std::thread::spawn(move || {
        let timer = SystemTime::now();
        s.time_suboptimal_search_solver(&problem, sender, stop, 2);
        println!("Optimality in: {:?}", timer.elapsed());
    });

    let solver = TEGSolver {
        time_step: chrono::Duration::new(60, 0).unwrap(),
        max_time_steps: chrono::Duration::new(80 * 60, 0).unwrap(),
        start: my_clock.now(),
    };

    let my_clock2 = my_clock.clone();
    let problem4 = problem3.clone();
    let child2 = std::thread::spawn(move || {
        let mut file = std::fs::File::create("perf.teg.txt").unwrap();
        let (sender, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let mut file2 = std::fs::File::create("time.teg.txt").unwrap();
        let timer = SystemTime::now();
        std::thread::spawn(move || {
            let soln = solver.solve_optimally(problem2, sender, stop);
            file2.write(format!("Opt Time {:?}\n", soln).as_bytes());
            file2.write(
                format!(
                    "Opt Time {:?}\n",
                    get_makespan(soln.unwrap(), problem4.clone()).unwrap() - my_clock2.now()
                )
                .as_bytes(),
            );
        });
        //
        for c in rx.iter() {
            let res = format!("TEG solution found in: {:?}\n", timer.elapsed());
            file.write(&res.as_bytes());
            /*match c {
                rmf_reservations::algorithms::AlgorithmState::FeasibleScheduleSolution(hash_map) => {file2.write(format!("Subopt Time {:?}\n", get_makespan(hash_map, problem4.clone()).unwrap() - my_clock2.now()).as_bytes());} ,
                rmf_reservations::algorithms::AlgorithmState::OptimalScheduleSolution(hash_map) =>  {file2.write(format!("Opt Time {:?}\n", get_makespan(hash_map, problem4.clone()).unwrap() - my_clock2.now()).as_bytes());},
                rmf_reservations::algorithms::AlgorithmState::OptimalSolution(hash_map) => todo!(),
                rmf_reservations::algorithms::AlgorithmState::PartialSolution(hash_map, _) => todo!(),
                rmf_reservations::algorithms::AlgorithmState::NotFound => todo!(),
                rmf_reservations::algorithms::AlgorithmState::UnSolveable => todo!(),
            };*/
        }
        let res = format!("Optimal TEG solution found in: {:?}\n", timer.elapsed());
        file.write(&res.as_bytes());
    });

    let timer = SystemTime::now();
    let mut file = std::fs::File::create("perf.txt").unwrap();
    let mut file2 = std::fs::File::create("time.txt").unwrap();
    for c in rx.iter() {
        let res = format!("Sub-optimal solution found in: {:?}\n", timer.elapsed());
        file.write(&res.as_bytes());

        match c {
            rmf_reservations::algorithms::AlgorithmState::FeasibleScheduleSolution(hash_map) => {
                file2.write(
                    format!(
                        "Subopt Time {:?}",
                        get_makespan(hash_map, problem3.clone()).unwrap() - my_clock.now()
                    )
                    .as_bytes(),
                );
            }
            rmf_reservations::algorithms::AlgorithmState::OptimalScheduleSolution(hash_map) => {
                file2.write(
                    format!(
                        "Opt Time {:?}",
                        get_makespan(hash_map, problem3.clone()).unwrap() - my_clock.now()
                    )
                    .as_bytes(),
                );
            }
            rmf_reservations::algorithms::AlgorithmState::OptimalSolution(hash_map) => todo!(),
            rmf_reservations::algorithms::AlgorithmState::PartialSolution(hash_map, _) => todo!(),
            rmf_reservations::algorithms::AlgorithmState::NotFound => todo!(),
            rmf_reservations::algorithms::AlgorithmState::UnSolveable => todo!(),
        };
    }
    let res = format!("Optimal found in: {:?}\n", timer.elapsed());
    file.write(&res.as_bytes());
    child2.join();
    let mut file3 = std::fs::File::create("greedy.time.txt").unwrap();
    let res = format!("FIFO: {:?}\n", lift_assigner.greedy_policy());
    file3.write(&res.as_bytes());
}

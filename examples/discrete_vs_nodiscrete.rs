use std::hint;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::{fs::OpenOptions, time::SystemTime};

use chrono::Duration;
use chrono::TimeZone;
use chrono::Utc;
use rmf_reservations::algorithms::greedy_solver::ConflictTracker;
use rmf_reservations::algorithms::greedy_solver::Problem;
use rmf_reservations::algorithms::sat::{generate_sat_devil, SATSolver};
use rmf_reservations::algorithms::sat_flexible_time_model;
use rmf_reservations::algorithms::sat_flexible_time_model::SATFlexibleTimeModel;
use rmf_reservations::cost_function::static_cost::StaticCost;
use rmf_reservations::database::ClockSource;
use rmf_reservations::discretization::fixed_timestep::FixedTimestep;
use rmf_reservations::discretization::DescretizationStrategy;
use rmf_reservations::ReservationParameters;
use rmf_reservations::ReservationRequest;
use rmf_reservations::StartTimeRange;

#[derive(Default, Clone)]
struct FakeClock;

impl ClockSource for FakeClock {
    fn now(&self) -> chrono::DateTime<chrono::prelude::Utc> {
        let time = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap();
        return time;
    }
}


fn generate_reservation_requests(
    time_range: chrono::Duration,
    alts: usize,
    reqs: usize,
) -> (Vec<Vec<ReservationRequest>>, Vec<String>) {
    let mut result = vec![];
    let mut rng = rand::thread_rng();

    let mut resource = vec![];
    for i in 0..alts {
        resource.push(format!("Station {}", i));
    }

    let start = Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap();
    let end = start + time_range;

    for req in 0..reqs {
        let mut alternatives = vec![];
        for alternative in 0..alts {
            let x = ReservationRequest {
                parameters: ReservationParameters {
                    resource_name: resource[alternative].clone(),
                    duration: Some(Duration::minutes(200)),
                    start_time: StartTimeRange {
                        earliest_start: Some(start),
                        latest_start: Some(end),
                    },
                },
                cost_function: Arc::new(StaticCost::new(1.0)),
            };
            alternatives.push(x);
        }
        result.push(alternatives);
    }

    (result, resource)
}

fn discretize(res: &Vec<Vec<ReservationRequest>>, clock_source: FakeClock) -> Vec<Vec<ReservationRequest>> {
    let mut latest = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap();
    for r in res {
        for v in r {
            if latest < v.parameters.start_time.latest_start.unwrap() {
                latest = v.parameters.start_time.latest_start.unwrap();
            }
        }
    }
    let mut remapper = FixedTimestep::new(Duration::minutes(10), latest, clock_source);
    remapper.discretize(res)
}


fn main() {
    //for x in 6..10 {
    for time_length in 2..20 {
        for _ in 0..100 {
            let (requests, resources) =
                generate_reservation_requests(chrono::Duration::hours(time_length * 4), 5, 5); //generate_sat_devil(x, x - 2);
                                                                                               //generate_test_scenario_with_known_best(5, 10, x);
                                                                                               //println!("Requests {:?}", requests);

            //
            let mut system = ConflictTracker::create_with_resources(&resources);
            let discrete_requests = discretize(&requests, FakeClock::default());

            for req in &discrete_requests {
                system.request_resources(req.clone());
                println!("{:?}", req.len());
            }
            let soln = system.generate_literals_and_remap_requests();

            let timer = SystemTime::now();
            SATSolver::without_optimality_check(soln.clone());
            let optimality_proof_dur = timer.elapsed();

            let sat_flexible_time_problem = sat_flexible_time_model::Problem { requests };

            let stop = Arc::new(AtomicBool::new(false));

            let timer = SystemTime::now();
            //SATSolver::from_hill_climber(soln.clone());\
            let model = SATFlexibleTimeModel {
                clock_source: FakeClock::default(),
            };

            model.feasbility_analysis(&sat_flexible_time_problem, stop);

            let greedy_dur = timer.elapsed();

            let mut file = OpenOptions::new()
                .write(true)
                .append(true)
                .open("running_log.txt")
                .unwrap();

            println!("Ran 1");
            writeln!(
                file,
                "{:?}, {:?},  {:?}",
                time_length, optimality_proof_dur, greedy_dur
            );
        }
    }
    //}
    // Ok(())
}

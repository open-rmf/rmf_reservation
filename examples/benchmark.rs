/// Benchmark
///
/// This benchmark compares the speed at which we can calculate suboptimal solutions, optimal solutions using the assignment heuristic and
///
use std::hint;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::{fs::OpenOptions, time::SystemTime};

use fnv::FnvHashMap;
use rmf_reservations::algorithms::greedy_solver::ConflictTracker;
use rmf_reservations::algorithms::sat::{generate_sat_devil, SATSolver};
use rmf_reservations::discretization;

fn main() {
    for x in (10..50).step_by(10) {
        for _ in 0..100 {
            let (requests, resources) = generate_sat_devil(x, x - 2);
            //generate_test_scenario_with_known_best(5, 10, x);
            //println!("Requests {:?}", requests);

            let mut system = ConflictTracker::create_with_resources(&resources);
            for req in requests {
                system.request_resources(req);
            }
            let soln = system.generate_literals_and_remap_requests();

            let timer = SystemTime::now();

            SATSolver::without_optimality_check(soln.clone());
            let optimality_proof_dur = timer.elapsed();

            let timer = SystemTime::now();
            SATSolver::from_hill_climber(soln.clone());
            let brute_force_proof_dur = timer.elapsed();

            let hint = FnvHashMap::default();
            let timer = SystemTime::now();

            let stop = Arc::new(AtomicBool::new(false));
            soln.solve(hint, stop);
            let greedy_dur = timer.elapsed();

            println!(
                "{} {:?} {:?} {:?}",
                x, optimality_proof_dur, brute_force_proof_dur, greedy_dur
            );

            let mut file = OpenOptions::new()
                .write(true)
                .append(true)
                .open("running_log.txt")
                .unwrap();

            writeln!(
                file,
                "{:?}, {:?}, {:?}, {:?}",
                x, optimality_proof_dur, brute_force_proof_dur, greedy_dur
            );
        }
    }
    // Ok(())
}

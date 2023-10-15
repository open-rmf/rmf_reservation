use std::{collections::HashMap, sync::{Arc, Mutex, mpsc::{Sender, self}, atomic::AtomicBool}, thread};

use chrono::Duration;

use self::greedy_solver::{Problem};

pub mod greedy_solver;
pub mod kuhn_munkres;
pub mod sat;

pub enum AlgorithmState {
    OptimalSolution(HashMap<usize, usize>),
    PartialSolution(HashMap<usize, usize>, f64),
    NotFound,
    UnSolveable
}


pub(crate) struct AlgorithmPool {
    proposed_solution: AlgorithmState,
    running: Arc<AtomicBool>,
    algorithms: Vec<Arc<dyn SolverAlgorithm + Send +Sync>>
}

impl Default for AlgorithmPool {
    fn default() -> Self {
        Self { 
            proposed_solution: AlgorithmState::NotFound, 
            running: Arc::new(AtomicBool::new(false)),
            algorithms: Vec::new()
        }
    }
}

impl AlgorithmPool {

    fn should_continue(&self) -> bool {
        match self.proposed_solution {
            AlgorithmState::OptimalSolution(_) => false,
            AlgorithmState::PartialSolution(_, _) => true,
            AlgorithmState::NotFound => true,
            AlgorithmState::UnSolveable => false,
        }
    }

    fn add_algorithm(&mut self, solver: Arc<dyn SolverAlgorithm + Send + Sync>) {
        self.algorithms.push(solver);
    }

    fn solve(&mut self, problem: Problem) {

        let (sender, rx) = mpsc::channel();
        let mut join_handles = vec![];
        for algorithm in &self.algorithms {
            let tx = sender.clone();
            let alg = algorithm.clone();
            let stop = self.running.clone();
            //TODO(arjoc) unessecary clone
            let problem = problem.clone();
            join_handles.push(thread::spawn(move || {
                alg.iterative_solve(tx, stop, problem)
            }));
        }
        loop {
            'solve: {
            let proposed_solution = rx.recv_timeout(std::time::Duration::from_millis(500));
            let Ok(soln) = proposed_solution else {
                if let Err(error) = proposed_solution {
                    match error {
                        mpsc::RecvTimeoutError::Timeout => {
                            if self.running.load(std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }
                            break 'solve;
                        }
                        mpsc::RecvTimeoutError::Disconnected => {
                            self.stop();
                            return;
                        }
                    }
                }
                //This is unreachable
                // TODO(Arjo): Refactor into match
                return;
            };
        

            if let AlgorithmState::PartialSolution(prev_soln, cost) = &self.proposed_solution {
                if let AlgorithmState::PartialSolution(_, cost2) = soln {
                    if cost2 < *cost {
                        self.proposed_solution = soln;
                    }
                }
                else {
                    self.proposed_solution = soln;
                }
            }
            else {
                self.proposed_solution = soln;
            }

            if !self.should_continue() {
                self.stop();
            }
        }
        }

    }

    fn stop(&mut self) {
        self.running.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}


pub trait SolverAlgorithm {
    fn iterative_solve(&self, result_channel: Sender<AlgorithmState>, stop: Arc<AtomicBool>, problem: Problem);
}

#[cfg(test)]
#[test]
fn test_sat() {
    use crate::{algorithms::greedy_solver::{GreedySolver, ConflictTracker}, scenario_generation::generate_test_scenario_with_known_best};

    use self::sat::SATSolver;

    let (requests, resources) = //generate_sat_devil(5,3);
    generate_test_scenario_with_known_best(10, 10, 5);
    //println!("Requests {:?}", requests);

    let mut system = ConflictTracker::create_with_resources(&resources);
    for req in requests {
        system.request_resources(req);
    }
    let problem = system.generate_literals_and_remap_requests();

    let mut pool = AlgorithmPool::default();
    pool.add_algorithm(Arc::new(SATSolver));
    pool.add_algorithm(Arc::new(GreedySolver));
    pool.solve(problem);
}

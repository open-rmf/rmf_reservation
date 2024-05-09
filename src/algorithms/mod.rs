//! This module provides you with a list of potential algorithms you may use to solve a reservation problem

use std::sync::mpsc::Receiver;
use std::{
    collections::HashMap,
    sync::{
        atomic::AtomicBool,
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use crate::database::Snapshot;
use chrono::Duration;

use self::{greedy_solver::Problem, sat_flexible_time_model::Assignment};

pub mod greedy_solver;
pub mod kuhn_munkres;
pub mod sat;
pub mod sat_flexible_time_model;

#[derive(Debug, Clone)]

pub enum AlgorithmState {
    FeasibleScheduleSolution(HashMap<String, Vec<Assignment>>),
    OptimalSolution(HashMap<usize, usize>),
    PartialSolution(HashMap<usize, usize>, f64),
    NotFound,
    UnSolveable,
}
pub(crate) struct AlgorithmPool<P> {
    proposed_solution: AlgorithmState,
    running: Arc<AtomicBool>,
    algorithms: Vec<Arc<dyn SolverAlgorithm<P> + Send + Sync>>,
}

impl<P> Default for AlgorithmPool<P> {
    fn default() -> Self {
        Self {
            proposed_solution: AlgorithmState::NotFound,
            running: Arc::new(AtomicBool::new(false)),
            algorithms: Vec::new(),
        }
    }
}

impl<P: Clone + std::marker::Send + 'static> AlgorithmPool<P> {
    fn clean_solver(&self) -> Self {
        let mut res = Self::default();
        res.algorithms = self.algorithms.clone();
        res
    }

    fn should_continue(&self) -> bool {
        match self.proposed_solution {
            AlgorithmState::FeasibleScheduleSolution(_) => false, //TODO make true
            AlgorithmState::OptimalSolution(_) => false,
            AlgorithmState::PartialSolution(_, _) => true,
            AlgorithmState::NotFound => true,
            AlgorithmState::UnSolveable => false,
        }
    }

    pub fn add_algorithm(&mut self, solver: Arc<dyn SolverAlgorithm<P> + Send + Sync>) {
        self.algorithms.push(solver);
    }

    fn solve(&mut self, problem: P, mtx: Arc<Mutex<AlgorithmState>>) {
        let (sender, rx) = mpsc::channel();
        let mut join_handles = vec![];
        for algorithm in &self.algorithms {
            let tx = sender.clone();
            let alg = algorithm.clone();
            let stop = self.running.clone();
            //TODO(arjoc) unessecary clone
            let p = problem.clone();
            join_handles.push(thread::spawn(move || alg.iterative_solve(tx, stop, p)));
        }
        'finished: loop {
            'solve: {
                let proposed_solution = rx.recv_timeout(std::time::Duration::from_millis(500));
                let Ok(soln) = proposed_solution else {
                    if let Err(error) = proposed_solution {
                        match error {
                            mpsc::RecvTimeoutError::Timeout => {
                                if self.running.load(std::sync::atomic::Ordering::Relaxed) {
                                    break 'finished;
                                }
                                break 'solve;
                            }
                            mpsc::RecvTimeoutError::Disconnected => {
                                self.stop();
                                break 'finished;
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
                    } else {
                        self.proposed_solution = soln;
                    }
                } else {
                    self.proposed_solution = soln;
                }

                *mtx.lock().unwrap() = self.proposed_solution.clone();

                if !self.should_continue() {
                    self.stop();
                }
            }
        }

        for handle in join_handles {
            handle.join();
        }
    }

    fn stop(&mut self) {
        self.running
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

struct ExecutionContext {
    join_handle: JoinHandle<()>,
    stop_handle: Arc<AtomicBool>,
}

pub(crate) struct AsyncExecutor<P, T> {
    execution_context: Option<Arc<Mutex<ExecutionContext>>>,
    algorithm_pool_template: AlgorithmPool<P>,
    solution: Arc<Mutex<AlgorithmState>>,
    metadata: T,
}

impl<P: Clone + std::marker::Send + 'static, T: Default + Clone> AsyncExecutor<P, T> {
    pub(crate) fn init(alg_pool: AlgorithmPool<P>) -> Self {
        Self {
            execution_context: None,
            algorithm_pool_template: alg_pool,
            solution: Arc::new(Mutex::new(AlgorithmState::NotFound)),
            metadata: T::default(),
        }
    }
    pub(crate) fn attempt_solve(&mut self, snapshot: Snapshot<P, T>) {
        self.metadata = snapshot.metadata;
        if let Some(context) = self.execution_context.clone() {
            let Ok(ref mut context) = context.lock() else {
                // Should not reach here
                return;
            };
            if !context.join_handle.is_finished() {
                context
                    .stop_handle
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let mut solver = self.algorithm_pool_template.clean_solver();
        let stop_handle = solver.running.clone();
        let solulu = self.solution.clone();
        let solver_thread = std::thread::spawn(move || {
            solver.solve(snapshot.problem, solulu);
        });
        self.execution_context = Some(Arc::new(Mutex::new(ExecutionContext {
            join_handle: solver_thread,
            stop_handle: stop_handle,
        })));
    }

    pub(crate) fn retrieve_best_fixed_time_solution_and_stop(
        &mut self,
    ) -> Option<HashMap<usize, usize>> {
        if let Some(context) = &self.execution_context {
            context
                .lock()
                .unwrap()
                .stop_handle
                .store(false, std::sync::atomic::Ordering::Relaxed);
        } else {
            return None;
        }

        let data = self.solution.lock().unwrap().clone();
        match &data {
            AlgorithmState::OptimalSolution(solution) => return Some(solution.clone()),
            AlgorithmState::PartialSolution(solution, _) => return Some(solution.clone()),
            AlgorithmState::FeasibleScheduleSolution(_) => return None,
            _ => {}
        };

        return None;
    }

    pub(crate) fn retrieve_feasible_schedule(
        &mut self,
    ) -> Option<(HashMap<String, Vec<Assignment>>, T)> {
        if let Some(context) = &self.execution_context {
            context
                .lock()
                .unwrap()
                .stop_handle
                .store(false, std::sync::atomic::Ordering::Relaxed);
        } else {
            return None;
        }

        let data = self.solution.lock().unwrap().clone();
        match &data {
            AlgorithmState::FeasibleScheduleSolution(solution) => {
                return Some((solution.clone(), self.metadata.clone()))
            }
            _ => {}
        };

        return None;
    }
}

pub trait SolverAlgorithm<P> {
    fn iterative_solve(
        &self,
        result_channel: Sender<AlgorithmState>,
        stop: Arc<AtomicBool>,
        problem: P,
    );
}

#[cfg(test)]
#[test]
fn test_multisolver_algorithm_pool() {
    use crate::{
        algorithms::greedy_solver::{ConflictTracker, GreedySolver},
        scenario_generation::generate_test_scenario_with_known_best,
    };

    use self::sat::FixedTimeSATSolver;

    let (requests, resources) = //generate_sat_devil(5,3);
    generate_test_scenario_with_known_best(10, 10, 5);
    //println!("Requests {:?}", requests);

    let mut system = ConflictTracker::create_with_resources(&resources);
    for req in requests {
        system.request_resources(req);
    }
    let problem = system.generate_literals_and_remap_requests();

    let mut pool = AlgorithmPool::default();
    pool.add_algorithm(Arc::new(FixedTimeSATSolver));
    pool.add_algorithm(Arc::new(GreedySolver));
    let mtx = Arc::new(Mutex::new(AlgorithmState::NotFound));
    pool.solve(problem, mtx);
}

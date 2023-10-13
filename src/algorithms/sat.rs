use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::Hash,
    sync::{Arc, mpsc::Sender, atomic::AtomicBool},
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use itertools::Itertools;
use varisat::{ExtendFormula, Lit, Solver};

use crate::{
    cost_function::{self, static_cost::StaticCost},
    ReservationRequest,
};

use super::{greedy_solver::Problem, AlgorithmState, SolverAlgorithm};

#[derive(Eq, PartialEq)]
struct AssumptionsHash {
    assumptions: fnv::FnvHashSet<Lit>,
}

impl AssumptionsHash {
    fn from_list(list: &AssumptionList) -> Self {
        Self {
            assumptions: fnv::FnvHashSet::from_iter(
                list.assumptions.iter().flatten().map(|f| f.clone()),
            ),
        }
    }
}

impl Hash for AssumptionsHash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for assumption in &self.assumptions {
            state.write_usize(assumption.index());
        }
    }
}

#[derive(Debug)]
struct AssumptionList {
    assumptions: Vec<Vec<Lit>>,
}

pub struct SATSolver;

impl SolverAlgorithm for SATSolver {
    fn iterative_solve(&self, result_channel: Sender<AlgorithmState>, stop: Arc<AtomicBool>, problem: Problem) {
        Self::from_hill_climber_with_optimality_proof(problem, result_channel, stop);
    }
}

impl SATSolver {
    pub fn from_hill_climber(problem: Problem) {
        let conflicts = problem.get_banned_reservation_combinations();
        let score_cache = problem.score_cache();
        let requests = problem.literals();

        let mut idx = 0usize;
        let mut var_list = HashMap::new();
        let mut idx_to_assignment = HashMap::new();

        let mut formula = varisat::CnfFormula::new();
        // Convert to CNF
        for (request, num_alt) in requests {
            let mut options = vec![];
            for i in 0..num_alt {
                idx += 1;
                let v = varisat::Var::from_index(idx);
                var_list.insert((request, i), v);
                idx_to_assignment.insert(idx, (request, i));
                options.push(v);
            }

            // These clauses state that there can be only one alternative chosen from the reservations
            let v: Vec<_> = options.iter().map(|v| Lit::from_var(*v, true)).collect();
            formula.add_clause(v.as_slice());

            for var_pair in options.iter().combinations(2) {
                if var_pair.len() != 2 {
                    panic!("Invalid combination found");
                }

                formula.add_clause(&[
                    Lit::from_var(*var_pair[0], false),
                    Lit::from_var(*var_pair[1], false),
                ]);
            }
        }

        // These clauses identify overlapping reservations
        for (key, conflicts) in conflicts.iter() {
            let Some(v1) = var_list.get(&key) else {
                continue;
            };

            for conflict in conflicts {
                if key == conflict {
                    continue;
                }

                let Some(v2) = var_list.get(conflict) else {
                    continue;
                };

                formula.add_clause(&[Lit::from_var(*v1, false), Lit::from_var(*v2, false)]);
            }
        }

        let mut solver = Solver::new();

        let mut bound = f64::INFINITY;
        let mut best_assignment = HashMap::new();

        let mut seen_assumptions = HashSet::new();

        solver.add_formula(&formula);

        let no_assumption = AssumptionList {
            assumptions: vec![],
        };
        let mut queue = VecDeque::new();

        queue.push_back(no_assumption);

        while let Some(assumptions) = queue.pop_front() {
            //println!("Assumptions being explored: {:?}", assumptions);

            for assumption in &assumptions.assumptions {
                solver.assume(assumption);
            }

            let Ok(k) = solver.solve() else {
                println!("Error occured while solving");
                return;
            };

            if !k {
                println!("No solution found");
                continue;
            }

            let Some(model) = solver.model() else {
                return;
            };

            let mut score = 0.0;
            let mut solution = HashMap::new();

            for lit in model {
                if !lit.is_positive() {
                    continue;
                }
                let Some(v) = idx_to_assignment.get(&lit.var().index()) else {
                    continue;
                };
                score += score_cache[v];
                solution.insert(v.0, v.1);
            }

            //println!("Score: {:?} for {:?}", score, solution);

            if score < bound {
                bound = score;
                best_assignment = solution.clone();
                println!(
                    "Found better solution with score {:?}. \n\t{:?}",
                    score, solution
                );
            }
            for sol in solution {
                let Some(var) = var_list.get(&sol) else {
                    continue;
                };

                // TODO(arjo): Only allow better solutions for each column: This should limit search space
                //  without needing to exploit
                let mut new_assumptions = assumptions.assumptions.clone();
                new_assumptions.extend([vec![Lit::from_var(*var, false)]]);

                // TODO(arjo) Aggressively prune this.
                let new_assum_list = AssumptionList {
                    assumptions: new_assumptions,
                };

                let hash = AssumptionsHash::from_list(&new_assum_list);

                if seen_assumptions.contains(&hash) {
                    //println!("Seen assumption, aggressively pruning");
                    continue;
                }
                seen_assumptions.insert(hash);
                //TODO(arjo) Is push_back better or push_front?
                queue.push_front(new_assum_list);
            }
        }
        println!("Final solution");
    }

    //================================================================================================================================================
    // This method is the most reliable
    pub fn from_hill_climber_with_optimality_proof(problem: Problem, sender: Sender<AlgorithmState>, stop: Arc<AtomicBool>) {
        let conflicts = problem.get_banned_reservation_combinations();
        let score_cache = problem.score_cache();
        let requests = problem.literals();

        let mut idx = 0usize;
        let mut var_list = HashMap::new();
        let mut idx_to_assignment = HashMap::new();

        let mut formula = varisat::CnfFormula::new();
        // Convert to CNF
        for (&request, &num_alt) in &requests {
            let mut options = vec![];
            for i in 0..num_alt {
                idx += 1;
                let v = varisat::Var::from_index(idx);
                var_list.insert((request, i), v);
                idx_to_assignment.insert(idx, (request, i));
                options.push(v);
            }

            // These clauses state that there can be only one alternative chosen from the reservations
            let v: Vec<_> = options.iter().map(|v| Lit::from_var(*v, true)).collect();
            formula.add_clause(v.as_slice());

            for var_pair in options.iter().combinations(2) {
                if var_pair.len() != 2 {
                    panic!("Invalid combination found");
                }

                formula.add_clause(&[
                    Lit::from_var(*var_pair[0], false),
                    Lit::from_var(*var_pair[1], false),
                ]);
            }
        }

        // These clauses identify overlapping reservations
        for (key, conflicts) in conflicts.iter() {
            let Some(v1) = var_list.get(&key) else {
                continue;
            };

            for conflict in conflicts {
                if key == conflict {
                    continue;
                }

                let Some(v2) = var_list.get(conflict) else {
                    continue;
                };

                formula.add_clause(&[Lit::from_var(*v1, false), Lit::from_var(*v2, false)]);
            }
        }

        let mut solver = Solver::new();

        let mut bound = f64::INFINITY;
        let mut best_assignment = HashMap::new();

        solver.add_formula(&formula);

        let no_assumption = AssumptionList {
            assumptions: vec![],
        };
        let mut queue = VecDeque::new();

        queue.push_back(no_assumption);

        while let Some(mut assumptions) = queue.pop_front() {
            //println!("Assumptions being explored: {:?}", assumptions);

            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            for assumption in &assumptions.assumptions {
                solver.add_clause(assumption);
            }

            let Ok(k) = solver.solve() else {
                return;
            };

            if !k {
                continue;
            }

            let Some(model) = solver.model() else {
                return;
            };

            let mut score = 0.0;
            let mut solution = HashMap::new();

            for lit in model {
                if !lit.is_positive() {
                    continue;
                }
                let Some(v) = idx_to_assignment.get(&lit.var().index()) else {
                    continue;
                };
                score += score_cache[v];
                solution.insert(v.0, v.1);
            }

            println!("Score: {:?} for {:?}", score, solution);

            if score < bound {
                bound = score;
                best_assignment = solution.clone();
                sender.send(AlgorithmState::PartialSolution(solution.clone(), score));
                println!(
                    "Found better solution with score {:?}. \n\t{:?}",
                    score, solution
                );
                // For the next solution at least one of the assignments has to be cheaper
                let mut new_clause = vec![];

                for (req, idx) in solution {
                    let Some(&curr_score) = score_cache.get(&(req, idx)) else {
                        continue;
                    };

                    let Some(&num_alt) = requests.get(&req) else {
                        continue;
                    };

                    // Find all cheaper alternative
                    for i in 0..num_alt {
                        let Some(&sc) = score_cache.get(&(req, i)) else {
                            continue;
                        };

                        if sc < curr_score {
                            let Some(v) = var_list.get(&(req,i)) else {
                                continue;
                            };

                            println!("Adding {:?} {:?}", req, i);

                            new_clause.push(v.lit(true));
                        }
                    }
                }
                assumptions.assumptions.push(new_clause);
                queue.push_front(assumptions);
            } else {
                let mut new_clause = vec![];

                // Eliminate option
                for (req, idx) in solution {
                    let Some(v) = var_list.get(&(req,idx)) else {
                        continue;
                    };

                    new_clause.push(v.lit(false));
                }

                assumptions.assumptions.push(new_clause);
                queue.push_front(assumptions);
            }
        }

        //println!("Final solution {:?}", best_assignment);
        if best_assignment.len() == 0 {
            sender.send(AlgorithmState::UnSolveable);
        }
        else {
            sender.send(AlgorithmState::OptimalSolution(best_assignment.clone()));
        }
    }

}

pub fn generate_sat_devil(
    n_resources: usize,
    n_alt: usize,
) -> (Vec<Vec<ReservationRequest>>, Vec<String>) {
    let mut sat_devil_resources: Vec<_> = (0..n_resources).map(|i| format!("{:?}", i)).collect();
    let time_step = Duration::seconds(100);
    let start_time = Utc.with_ymd_and_hms(2014, 7, 8, 9, 10, 11).unwrap();
    let mut requests = vec![vec![]; n_resources];

    for i in 0..n_resources {
        for j in 0..n_alt {
            let req = ReservationRequest {
                parameters: crate::ReservationParameters {
                    resource_name: sat_devil_resources[(i + j) % n_resources].clone(),
                    duration: Some(Duration::seconds(100)),
                    start_time: crate::StartTimeRange::exactly_at(&start_time),
                },
                cost_function: Arc::new(StaticCost::new(j as f64)),
            };

            requests[i].push(req);
        }
    }
    (requests, sat_devil_resources)
}

#[cfg(test)]
#[test]
fn test_sat() {
    use std::{time::SystemTime, sync::mpsc};

    use fnv::FnvHashMap;

    use crate::scenario_generation::generate_test_scenario_with_known_best;

    use super::greedy_solver::ConflictTracker;

    let (requests, resources) = //generate_sat_devil(5,3);
        generate_test_scenario_with_known_best(10, 10, 5);
    //println!("Requests {:?}", requests);

    let mut system = ConflictTracker::create_with_resources(&resources);
    for req in requests {
        system.request_resources(req);
    }
    let soln = system.generate_literals_and_remap_requests();

    let timer = SystemTime::now();
    let (sender, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false)); 
    SATSolver::from_hill_climber_with_optimality_proof(soln.clone(), sender, stop);
    let optimality_proof_dur = timer.elapsed();
}

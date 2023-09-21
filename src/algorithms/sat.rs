use std::{collections::{HashMap, VecDeque}, hash::Hash};

use itertools::Itertools;
use varisat::{ExtendFormula, Lit, Solver};

use super::hierarchical_kuhn_munkres::SparseScheduleConflictHillClimber;


struct AssumptionList {
    assumptions: Vec<Vec<Lit>>,
}

struct SATSolver {

}

impl SATSolver {
    fn from_hill_climber(problem: SparseScheduleConflictHillClimber) {
        let conflicts = problem.get_banned_reservation_combinations();
        let score_cache = problem.score_cache();
        let requests = problem.literals();

        let mut idx = 0usize;
        let mut var_list = HashMap::new();
        let mut idx_to_assignment = HashMap::new();

        let mut formula = varisat::CnfFormula::new();
        // Convert to CNF
        for (request, num_alt) in  requests {
            let mut options = vec!();
            for i in 0..num_alt {
                idx+=1;
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
                    Lit::from_var(*var_pair[1], false)]);
            }
        }

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

                formula.add_clause(&[
                    Lit::from_var(*v1, false), 
                    Lit::from_var(*v2, false)
                ]);
            }
        }

        let mut solver = Solver::new();

        let mut bound = f64::INFINITY;
        let mut best_assignment = HashMap::new();
        
        solver.add_formula(&formula);

        let no_assumption = AssumptionList {
            assumptions: vec![]
        };
        let mut queue = VecDeque::new();

        queue.push_back(no_assumption);


        while let Some(assumptions) = queue.pop_front(){

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
                solution.insert(v.0 , v.1);
            }

            if score < bound {
                bound = score;
                best_assignment = solution.clone();
                println!("Found better solution with score {:?}. \n\t{:?}", score, solution);
            }
            for sol in solution {
                let Some(var) = var_list.get(&sol) else {
                    continue;
                };

                let mut new_assumptions = assumptions.assumptions.clone();
                new_assumptions.extend([vec![Lit::from_var(*var, false)]]);
                let new_assum_list = AssumptionList {
                    assumptions: new_assumptions
                };

                //TODO(arjo) Is push_back better or push_front?
                queue.push_front(new_assum_list);
            }
        }
        println!("");
    }
}

#[cfg(test)]
#[test]
fn test_sat() {
    use crate::algorithms::hierarchical_kuhn_munkres::generate_test_scenario_with_known_best;

    use super::hierarchical_kuhn_munkres::TimeBasedBranchAndBound;

    let (requests, resources) =  generate_test_scenario_with_known_best(10, 15, 40);
    //println!("Requests {:?}", requests);
    
    let mut system = TimeBasedBranchAndBound::create_with_resources(&resources);
    for req in requests {
        system.request_resources(req);
    }
    let soln = system.generate_literals_and_remap_requests();
    SATSolver::from_hill_climber(soln);
}
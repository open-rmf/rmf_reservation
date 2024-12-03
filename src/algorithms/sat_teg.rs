use std::collections::{HashMap, HashSet};

use chrono::Utc;
use itertools::Itertools;
use varisat::{ExtendFormula, Lit, Solver, Var};

use super::sat_flexible_time_model::{Assignment, Problem};

/// Solve resource scheduling problems with
/// time-expansions taken into mind.
pub struct TEGSolver {
    time_step: chrono::Duration,
    max_time_steps: chrono::Duration,
    start: chrono::DateTime<Utc>,
}

impl TEGSolver {
    fn to_time_idx(&self, time: chrono::DateTime<Utc>) -> usize {
        ((time - self.start).num_seconds() / self.time_step.num_seconds()) as usize
    }

    fn from_time_idx(&self, t: usize) -> chrono::DateTime<Utc> {
        self.start + self.time_step * (t as i32)
    }

    fn from_duration_to_indices(&self, duration: &Option<chrono::Duration>) -> i64 {
        if let Some(duration) = duration {
            (duration.num_seconds() / self.time_step.num_seconds())
        } else {
            (self.max_time_steps.num_seconds() / self.time_step.num_seconds())
        }
    }

    pub fn solve(&self, problem: Problem) -> Result<HashMap<String, Vec<Assignment>>, String> {
        //let decision_variables = HashMap::new();
        let mut resources = HashMap::new();
        let mut idx_to_res = Vec::new();
        for r in 0..problem.requests.len() {
            let r = problem.requests[r].clone();
            for alt in r {
                if resources.get(&alt.parameters.resource_name).is_some() {
                    continue;
                }
                resources.insert(alt.parameters.resource_name.clone(), idx_to_res.len());
                idx_to_res.push(alt.parameters.resource_name.clone());
            }
        }

        let max_time_idx = self.max_time_steps.num_seconds() / self.time_step.num_seconds();

        // Decisiont variables are a matrix of time x resources x requests x alternatives.
        let mut decision_vars = vec![];
        let mut idx = 0;
        let mut idx_to_alternative = HashMap::new();
        let mut idx_to_time_idx = HashMap::new();

        let mut formula = varisat::CnfFormula::new();

        let mut request_id_to_resource_index = HashMap::new();
        let mut alt_id_to_indices= HashMap::new();

        // Build the time expansion graph
        for t in 0..max_time_idx {
            let mut time_axis = vec![];
            for resource in 0..idx_to_res.len() {
                let mut awarded_res = vec![];
                for (req_id, p) in problem.requests.iter().enumerate() {
                    let mut awarded_alt = vec![];
                    for (alt_id, req) in p.iter().enumerate() {
                       
                        if req.parameters.resource_name != idx_to_res[resource] {
                            continue;
                        }
                        println!("{:?} {:?} => {:?} {:?}", req_id, alt_id, idx_to_res[resource], resource);
                        request_id_to_resource_index.insert((req_id, alt_id), (awarded_res.len(), awarded_alt.len()));
                        alt_id_to_indices.insert((resource, awarded_res.len(), awarded_alt.len()), (req_id, alt_id));
                        awarded_alt.push(Var::from_index(idx));
                        idx_to_alternative.insert(idx, (req_id, alt_id));
                        idx_to_time_idx.insert(idx, t);
                        idx += 1;
                    }
                    // mutex clause
                    for xy in awarded_alt.iter().combinations(2) {
                        formula.add_clause(&[
                            Lit::from_var(*xy[0], false),
                            Lit::from_var(*xy[1], false),
                        ]);
                    }
                    
                    awarded_res.push(awarded_alt)
                }

                // Mutex for resources
                // mutex clause.
                for xy in awarded_res.iter().flatten().combinations(2) {
                    formula
                        .add_clause(&[Lit::from_var(*xy[0], false), Lit::from_var(*xy[1], false)]);
                }
                time_axis.push(awarded_res);
            }
            decision_vars.push(time_axis);
        }

        for req_id in 0..problem.requests.len() {
            let p = problem.requests[req_id].clone();
            // At least one of the alternatives must be true
            let mut or_clause = vec![];
            for (alt_id, req) in p.iter().enumerate() {
                let Some(res_id) = resources.get(&req.parameters.resource_name) else {
                    continue;
                };
                for t in 0..max_time_idx {
                    println!("{:?}", (res_id, req_id, alt_id, req.parameters.resource_name.clone()));
                    let Some(indices) = request_id_to_resource_index.get(&(req_id, alt_id)) else {
                        return Err("Inconsitency in internal structures".to_string());
                    };
                    let d_var = decision_vars[t as usize][*res_id][indices.0][indices.1];
                    if req.falls_within_acceptable_time(&self.from_time_idx(t as usize)) {
                        or_clause.push(Lit::from_var(d_var, true));
                    }
                }
            }
            formula.add_clause(&or_clause);

            for (alt_id, req) in p.iter().enumerate() {
                let Some(res_id) = resources.get(&req.parameters.resource_name) else {
                    println!("Could not get resource {:?}", req.parameters.resource_name);
                    continue;
                };
                println!("resource {:?} -> {:?}", req.parameters.resource_name, res_id);
                // Mark allowed time range
                for t in 0..max_time_idx {
                    let Some(indices) = request_id_to_resource_index.get(&(req_id, alt_id)) else {
                        return Err("Inconsitency in internal structures".to_string());
                    };
                    let d_var = decision_vars[t as usize][*res_id][indices.0][indices.1];
                    if !req.falls_within_acceptable_time(&self.from_time_idx(t as usize)) {
                        formula.add_clause(&[Lit::from_var(d_var, false)]);
                        println!(
                            "Not allowed {:?}",
                            self.from_time_idx(t as usize) - self.start
                        );
                    }
                }

                for (i, j) in (0..max_time_idx).tuple_windows() {
                    let Some(indices) = request_id_to_resource_index.get(&(req_id, alt_id)) else {
                        return Err("Inconsitency in internal structures".to_string());
                    };
                    let i_var = decision_vars[i as usize][*res_id][indices.0][indices.1];
                    let j_var = decision_vars[j as usize][*res_id][indices.0][indices.1];
                    let duration = problem.requests[req_id][alt_id].parameters.duration;
                    if !req.satisfies_request(&self.from_time_idx(j as usize), duration) {
                        formula
                            .add_clause(&[Lit::from_var(i_var, true), Lit::from_var(j_var, false)]);
                        continue;
                    }

                    // Mark duration only if we can fit it in time window

                    // For most cases this would work, unless the reservation starts at x0
                    // (~x_{t-1} \land x_t) => (x_{t+1} \land x_{t+2}.... \land x_{t+dur})
                    if j + self.from_duration_to_indices(&duration) < max_time_idx {
                        println!("{:?} marking next few items {:?}", duration, j);
                        for k in
                            j + 1..j + self.from_duration_to_indices(&duration)
                        {
                            let k_var = decision_vars[k as usize][*res_id][indices.0][indices.1];
                            // This is the duration itself
                            formula.add_clause(&[
                                Lit::from_var(i_var, true),
                                Lit::from_var(j_var, false),
                                Lit::from_var(k_var, true),
                            ]);
                        }

                        // This handles the transition time by adding an implication if starts at
                        // x_{t}
                        for res in  0..decision_vars[j as usize].len() {
                            for m in 0..decision_vars[j as usize][res].len() 
                            {
                                for n in 0..decision_vars[j as usize][res][m].len() {

                                    let Some(other_req) = alt_id_to_indices.get(&(res,m,n)) else {
                                        panic!("should never get here");
                                    };
                                    
                                    let end_of_last = j + self.from_duration_to_indices(&duration);
                                    let Some(earliest_start_for_next) = problem.min_delay.get(&((req_id, alt_id), *other_req)) else {
                                        continue;
                                    };


                                    let earliest_start_for_next= end_of_last + self.from_duration_to_indices(&Some(*earliest_start_for_next));
                                    for k in
                                        end_of_last..earliest_start_for_next.min(max_time_idx)
                                    {
                                        let k_var = decision_vars[k as usize][res][m][n];
                                        // This is the duration itself
                                        formula.add_clause(&[
                                            Lit::from_var(i_var, true),
                                            Lit::from_var(j_var, false),
                                            Lit::from_var(k_var, false),
                                        ]);
                                    }
                                }
                            }  
                        }
                        
                    }
                    // Otherwise its too late
                    else {
                        println!("{:?} is too long to start from {:?}", duration, j);
                        formula
                            .add_clause(&[Lit::from_var(i_var, true), Lit::from_var(j_var, false)]);
                    }
                }
            }
        }

        let mut solver = Solver::new();
        solver.add_formula(&formula);
        let Ok(ok) = solver.solve() else {
            return Err("Hello".to_string());
        };
        if !ok {
            return Err("No solution found".to_string());
        }
        let Some(model) = solver.model() else {
            return Err("Unable to get model".to_string());
        };

        // Reconstruct schedule
        let mut earliest_start_and_res: HashMap<(usize, usize), i64> = HashMap::new();
        for lit in model {
            if !lit.is_positive() {
                continue;
            }
            let Some(p) = idx_to_alternative.get(&lit.index()) else {
                return Err("Got a decision var that is not in our original list. This is an eror that should never happen".to_string());
            };
            println!(
                ": {:?} {:?}",
                idx_to_time_idx[&lit.index()],
                self.from_time_idx(idx_to_time_idx[&lit.index()] as usize) - self.start
            );
            if let Some(time) = earliest_start_and_res.get_mut(p) {
                let var_time = idx_to_time_idx[&lit.index()];
                if var_time < *time {
                    *time = var_time;
                }
            } else {
                earliest_start_and_res.insert(*p, idx_to_time_idx[&lit.index()]);
            }
        }

        let mut unordered_schedule: HashMap<String, Vec<Assignment>> = HashMap::new();
        for (alternative, time) in earliest_start_and_res {
            let resource = problem.requests[alternative.0][alternative.1]
                .parameters
                .resource_name
                .clone();
            if let Some(sched) = unordered_schedule.get_mut(&resource) {
                sched.push(Assignment {
                    id: alternative.clone(),
                    start_time: self.from_time_idx(time as usize),
                });
            } else {
                unordered_schedule.insert(
                    resource.clone(),
                    vec![Assignment {
                        id: alternative.clone(),
                        start_time: self.from_time_idx(time as usize),
                    }],
                );
            }
        }
        for (_, vec) in unordered_schedule.iter_mut() {
            vec.sort_by(|a, b| a.start_time.cmp(&b.start_time))
        }

        Ok(unordered_schedule)
    }
}

#[cfg(test)]
#[test]
fn test_teg_one_item_teg_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::ReservationRequestAlternative;

    let current_time = chrono::Utc::now();

    let req1 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource1".to_string(),
            duration: Some(chrono::Duration::seconds(100)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(100)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];

    let problem = Problem {
        requests: vec![req1],
        ..Default::default()
    };

    let solver = TEGSolver {
        time_step: chrono::Duration::new(50, 0).unwrap(),
        max_time_steps: chrono::Duration::new(500, 0).unwrap(),
        start: current_time,
    };

    let soln = solver.solve(problem);
    println!("{:?}", soln);
    assert!(soln.is_ok());
}

#[cfg(test)]
#[test]
fn test_teg_one_item_teg_solver_too_small() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::ReservationRequestAlternative;

    let current_time = chrono::Utc::now();

    let req1 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource1".to_string(),
            duration: Some(chrono::Duration::seconds(600)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(100)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];

    let problem = Problem {
        requests: vec![req1],
        ..Default::default()
    };

    let solver = TEGSolver {
        time_step: chrono::Duration::new(50, 0).unwrap(),
        max_time_steps: chrono::Duration::new(500, 0).unwrap(),
        start: current_time,
    };

    let soln = solver.solve(problem);
    println!("{:?}", soln);
    assert!(soln.is_err());
}

#[cfg(test)]
#[test]
fn test_multi_alternative_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;
    use crate::ReservationRequestAlternative;

    let current_time = chrono::Utc::now();

    let req1 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource1".to_string(),
            duration: Some(chrono::Duration::seconds(100)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(120)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];

    let req2 = vec![
        ReservationRequestAlternative {
            parameters: crate::ReservationParameters {
                resource_name: "Resource1".to_string(),
                duration: Some(chrono::Duration::seconds(100)),
                start_time: crate::StartTimeRange {
                    earliest_start: Some(current_time + chrono::Duration::seconds(150)),
                    latest_start: Some(current_time + chrono::Duration::seconds(160)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        },
        ReservationRequestAlternative {
            parameters: crate::ReservationParameters {
                resource_name: "Resource2".to_string(),
                duration: Some(chrono::Duration::seconds(100)),
                start_time: crate::StartTimeRange {
                    earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                    latest_start: Some(current_time + chrono::Duration::seconds(160)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        },
    ];

    let mut problem = Problem::default();
    problem.request_one_of(req1);
    problem.request_one_of(req2);

    let solver = TEGSolver {
        time_step: chrono::Duration::new(50, 0).unwrap(),
        max_time_steps: chrono::Duration::new(500, 0).unwrap(),
        start: current_time,
    };

    let soln = solver.solve(problem);
    println!("{:?}", soln);

    /*let (sender, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .time_optimality_linear_search_solver(&problem, sender, stop);
    let mut v = vec![];
    for t in rx.iter() {
        v.push(t);
    }

    let Some(last) = v.last() else {
        panic!("Unable to get any solution");
    };

    let AlgorithmState::OptimalScheduleSolution(sched) = last else {
        panic!("Optimal solution was not found");
    };

    assert_eq!(sched["Resource1"][0].id, (0usize, 0usize));
    assert_eq!(sched["Resource1"].len(), 1usize);
    assert_eq!(sched["Resource2"][0].id, (1usize, 1usize));
    assert_eq!(sched["Resource2"].len(), 1usize);
    assert_eq!(sched.len(), 2usize);*/
}
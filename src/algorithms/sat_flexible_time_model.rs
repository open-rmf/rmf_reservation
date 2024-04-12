use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{atomic::AtomicBool, mpsc::Sender},
};

use itertools::Itertools;
use petgraph::{algo::toposort, Graph};
use varisat::{CnfFormula, ExtendFormula, Lit, Solver, Var};

use chrono::{prelude::*, Duration};

use crate::database::ClockSource;
use crate::ReservationRequest;

use super::{AlgorithmState, SolverAlgorithm};

#[derive(Debug, Clone)]
pub struct Problem {
    pub requests: Vec<Vec<ReservationRequest>>,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub id: (usize, usize),
    pub start_time: chrono::DateTime<Utc>,
}

#[cfg(test)]
fn check_consistency(assignments: &Vec<Assignment>, problem: &Problem) -> bool {
    if assignments.len() == 0 {
        return true;
    }

    let mut last_end = Some(assignments[0].start_time);

    for assignment in assignments {
        let req = &problem.requests[assignment.id.0][assignment.id.1];
        if let Some(duration) = req.parameters.duration {
            if let Some(last_end_time) = last_end {
                if last_end_time > assignment.start_time {
                    return false;
                }
                last_end = Some(assignment.start_time + duration);
            } else {
                return false;
            }
        } else {
            last_end = None;
        }
    }

    return true;
}

fn shrink_reservation_request(reservation_req: &ReservationRequest, time_window: DateTime<Utc>) -> Option<ReservationRequest>
{
    if let Some(earliest_start) = reservation_req.parameters.start_time.earliest_start {
        if earliest_start > time_window {
            return None;
        }
    }

    if let Some(latest_start) = reservation_req.parameters.start_time.latest_start {
        if latest_start < time_window {
            return Some(reservation_req.clone());
        }
    }

    Some(ReservationRequest {
        parameters: crate::ReservationParameters {
            resource_name: reservation_req.parameters.resource_name.clone(),
            duration: reservation_req.parameters.duration.clone(),
            start_time: crate::StartTimeRange {
                earliest_start: reservation_req.parameters.start_time.earliest_start.clone(),
                latest_start: Some(time_window)
            }
        },
        cost_function: reservation_req.cost_function.clone(),
    })
}

#[cfg(test)]
#[test]
fn test_shrink_reservation()
{

}


pub struct SATFlexibleTimeModel<CS: ClockSource + std::marker::Send + std::marker::Sync> {
    pub clock_source: CS,
}

#[derive(Debug, Clone, Copy)]
pub enum FlexibleSatError {
    TimedOut,
    NoSolution,
}

impl<CS: ClockSource + Clone + std::marker::Send + std::marker::Sync> SolverAlgorithm<Problem>
    for SATFlexibleTimeModel<CS>
{
    fn iterative_solve(
        &self,
        result_channel: std::sync::mpsc::Sender<super::AlgorithmState>,
        stop: std::sync::Arc<AtomicBool>,
        problem: Problem,
    ) {
        let Ok(problem) = self.feasbility_analysis(&problem, stop) else {
            result_channel.send(AlgorithmState::UnSolveable);
            return;
        };

        result_channel.send(AlgorithmState::FeasibleScheduleSolution(problem));
    }
}

impl<CS: ClockSource + Clone + std::marker::Send + std::marker::Sync> SATFlexibleTimeModel<CS> {

    pub fn time_optimality_solver(
        &self,
        problem: &Problem,
        sender: Sender<AlgorithmState>,
        stop: std::sync::Arc<AtomicBool>)
    {
        let mut resources = HashMap::new();
        let mut id_to_resource = vec![];
        let mut var_list = HashMap::new();
        let mut idx_to_option = vec![];

        let mut formula = varisat::CnfFormula::new();

        let mut var_by_resource = HashMap::new();

        let mut final_schedule = HashMap::new();

        for req_id in 0..problem.requests.len() {
            let mut options = vec![];
            let request_alternatives = &problem.requests[req_id];
            for alt_id in 0..request_alternatives.len() {
                let request = &request_alternatives[alt_id];
                if !resources.contains_key(&request.parameters.resource_name) {
                    resources.insert(
                        request.parameters.resource_name.clone(),
                        id_to_resource.len(),
                    );
                    var_by_resource.insert(id_to_resource.len(), vec![]);
                    id_to_resource.push(request.parameters.resource_name.clone());
                }
                let v = Var::from_index(idx_to_option.len());
                idx_to_option.push((req_id, alt_id));
                var_list.insert((req_id, alt_id), v);

                //NOTE: if this line panics something is  v weird. TODO(arjoc) reformat so impossible topanic.
                let mut option_list = var_by_resource
                    .get_mut(resources.get(&request.parameters.resource_name).unwrap());
                let Some(varlist) = option_list else {
                    panic!("We shouldnt reach here");
                };
                varlist.push((req_id, alt_id));
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

        let mut idx = idx_to_option.len();
        let mut comes_after_vars = HashMap::new();

        let mut idx_to_order = HashMap::new();
        // Strict total order variables
        for (_, alternatives) in var_by_resource.iter() {
            for i in 0..alternatives.len() {
                for j in 0..alternatives.len() {
                    if i == j {
                        continue;
                    }

                    let v = Var::from_index(idx);
                    idx_to_order.insert(idx, (alternatives[i], alternatives[j]));
                    idx += 1;

                    if !comes_after_vars.contains_key(&alternatives[i]) {
                        comes_after_vars.insert(alternatives[i], HashMap::new());
                    }
                    let Some(m) = comes_after_vars.get_mut(&alternatives[i]) else {
                        panic!("Should never reach here");
                    };
                    m.insert(alternatives[j], v);
                }
            }
        }

        // Strict Total Order constraints
        for (_, alternatives) in var_by_resource.iter() {
            for i in 0..alternatives.len() {
                for j in i + 1..alternatives.len() {
                    let ij = alternatives[i];
                    let km = alternatives[j];
                    let X_ijkm = comes_after_vars
                        .get(&ij)
                        .unwrap()
                        .get(&km)
                        .expect("something went wrong");
                    let X_kmij = comes_after_vars
                        .get(&km)
                        .unwrap()
                        .get(&ij)
                        .expect("something went wrong");
                    let x_ij = var_list.get(&ij).expect("Something went wrong");
                    let x_km = var_list.get(&km).expect("Something went wrong");

                    // Assymmetry
                    formula.add_clause(&[
                        Lit::from_var(*x_ij, false),
                        Lit::from_var(*x_km, false),
                        Lit::from_var(*X_ijkm, false),
                        Lit::from_var(*X_kmij, false),
                    ]);

                    // Connectedness
                    formula.add_clause(&[
                        Lit::from_var(*x_ij, false),
                        Lit::from_var(*x_km, false),
                        Lit::from_var(*X_ijkm, true),
                        Lit::from_var(*X_kmij, true),
                    ])
                }
            }

            // Transitivity (Warning O(n^3))
            for (_ij, x_ij_) in comes_after_vars.iter() {
                for (km, X_ijkm) in x_ij_.iter() {
                    let Some(other) = comes_after_vars.get(km) else {
                        continue;
                    };
                    for (nl, X_kmnl) in other.iter() {
                        let Some(X_ijnl) = x_ij_.get(nl) else {
                            //panic!("Failed to get {:?}", nl);
                            continue;
                        };

                        formula.add_clause(&[
                            Lit::from_var(*X_ijkm, false),
                            Lit::from_var(*X_kmnl, false),
                            Lit::from_var(*X_ijnl, true),
                        ]);
                    }
                }
            }
        }

        // Prededuced constraints based on scheduling constraints
        for (_, alternatives) in var_by_resource.iter() {
            for i in 0..alternatives.len() {
                for j in i + 1..alternatives.len() {
                    let alt_ij = alternatives[i];
                    let alt_km = alternatives[j];

                    let alt_ij_original = &problem.requests[alt_ij.0][alt_ij.1];
                    let alt_km_original = &problem.requests[alt_km.0][alt_km.1];

                    let Some(list_ij) = comes_after_vars.get(&alt_ij) else {
                        panic!("For some reason");
                    };

                    let X_ijkm = list_ij.get(&alt_km).expect("");
                    let Some(list_km) = comes_after_vars.get(&alt_km) else {
                        panic!("For some reason");
                    };

                    let X_kmij = list_km.get(&alt_ij).expect("");
                    if !alt_ij_original.can_be_scheduled_after(&alt_km_original.parameters) {
                        // ij cannot be after km
                        formula.add_clause(&[Lit::from_var(*X_ijkm, false)]);
                    }

                    if !alt_km_original.can_be_scheduled_after(&alt_ij_original.parameters) {
                        // ij cannot be after km
                        formula.add_clause(&[Lit::from_var(*X_kmij, false)]);
                    }
                }
            }
        }

        let mut solver = Solver::new();
        solver.add_formula(&formula);

        let mut solved = false;

        let current_time = self.clock_source.now();

        let mut time_window = None;

        while !solved {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                sender.send(AlgorithmState::NotFound);
                return;
            }

            final_schedule.clear();


            if let Some(time_window) = time_window
            {
                let mut formula = varisat::CnfFormula::new();
                for (_, alternatives) in var_by_resource.iter() {
                    for i in 0..alternatives.len() {
                        for j in i + 1..alternatives.len() {
                            let alt_ij = alternatives[i];
                            let alt_km = alternatives[j];

                            let alt_ij_shrink = shrink_reservation_request(&problem.requests[alt_ij.0][alt_ij.1], time_window);
                            let alt_km_shrink = shrink_reservation_request(&problem.requests[alt_km.0][alt_km.1], time_window);

                            if alt_ij_shrink.is_none() {
                                // Ban the entire alternative
                                let x_ij = var_list.get(&alt_ij).expect("Something went wrong");
                                formula.add_clause(&[Lit::from_var(*x_ij, false)]);

                            }

                            if alt_ij_shrink.is_none() {
                                // Ban the entire alternative
                                let x_km = var_list.get(&alt_km).expect("Something went wrong");
                                formula.add_clause(&[Lit::from_var(*x_km, false)]);
                            }

                            if let Some(alt_ij_shrink) = alt_ij_shrink{
                                if let Some(alt_km_shrink) = alt_km_shrink {
                                    let Some(list_ij) = comes_after_vars.get(&alt_ij) else {
                                        panic!("For some reason");
                                    };

                                    let X_ijkm = list_ij.get(&alt_km).expect("");
                                    let Some(list_km) = comes_after_vars.get(&alt_km) else {
                                        panic!("For some reason");
                                    };

                                    let X_kmij = list_km.get(&alt_ij).expect("");
                                    if !alt_ij_shrink.can_be_scheduled_after(&alt_km_shrink.parameters) {
                                        // ij cannot be after km
                                        formula.add_clause(&[Lit::from_var(*X_ijkm, false)]);
                                    }

                                    if !alt_km_shrink.can_be_scheduled_after(&alt_ij_shrink.parameters) {
                                        // ij cannot be after km
                                        formula.add_clause(&[Lit::from_var(*X_kmij, false)]);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            solver.solve();

            let Ok(k) = solver.solve() else {
                println!("Failed to solve");
                break;
            };

            if !k {
                println!("No soln");
                break;
            }

            let Some(model) = solver.model() else {
                break;
            };

            let mut edges = vec![];
            let mut vertices = vec![];
            for lit in model {
                if !lit.is_positive() {
                    continue;
                }
                let v = lit.var();
                let v_idx = v.index();

                if let Some((from, to)) = idx_to_order.get(&v_idx) {
                    edges.push(((*from), (*to)));
                } else {
                    if v_idx >= idx_to_option.len() {
                        continue;
                    }

                    let vert = idx_to_option[v_idx];
                    vertices.push(vert)
                }
            }

            // Build dependency graph
            let mut pgraph = Graph::<(usize, usize), bool>::new();
            let mut node_map = HashMap::new();

            for v in vertices {
                node_map.insert(v, pgraph.add_node(v));
            }
            for (after, before) in edges {
                pgraph.add_edge(
                    *node_map.get(&after).unwrap(),
                    *node_map.get(&before).unwrap(),
                    true,
                );
            }
            let Ok(res) = toposort(&pgraph, None) else {
                panic!("Sometthing wrong with SAT formula found cycle.");
            };
            let order: Vec<_> = res
                .iter()
                .map(|v| pgraph.raw_nodes()[v.index()].weight)
                .collect();
            let mut schedules: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

            for res_pair in order {
                let resource = &problem.requests[res_pair.0][res_pair.1]
                    .parameters
                    .resource_name;

                if let Some(sched) = schedules.get_mut(resource) {
                    sched.push(res_pair);
                } else {
                    schedules.insert(resource.clone(), vec![res_pair]);
                }
            }

            //println!("Schedule: {:?}", schedules);

            let mut learned_clauses = vec![];
            let mut ok = true;

            // Solve time slots. Can be parallelized.
            for (res_name, sched) in schedules {
                let mut last_reservation_end = current_time;
                let mut last_gap = 0usize;
                final_schedule.insert(res_name.clone(), vec![]);
                let Some(resource_schedule) = final_schedule.get_mut(&res_name) else {
                    panic!("Should never reach here")
                };
                for i in 0..sched.len() {
                    let alternative = &problem.requests[sched[i].0][sched[i].1];

                    let Some(duration) = alternative.parameters.duration else {
                        if i + 1 == sched.len() {
                            if let Some(latest) = alternative.parameters.start_time.latest_start {
                                if last_reservation_end > latest {
                                    // TODO(back track)
                                    println!("Timed out {}, {}", latest, last_reservation_end);
                                    let mut formula = vec![];
                                    for j in last_gap..i {
                                        let v = var_list
                                            .get(&sched[j])
                                            .expect("Could not get reservation end");
                                        formula.push(Lit::from_var(*v, false));
                                    }
                                    learned_clauses.push(formula);
                                    ok = false;
                                }
                            }
                            continue;
                        } else {
                            panic!("Somehow ended up with an infinite reservation with no end in the middle of the schedule")
                        }
                    };

                    if let Some(earliest) = alternative.parameters.start_time.earliest_start {
                        if earliest >= last_reservation_end {
                            resource_schedule.push(Assignment {
                                id: sched[i].clone(),
                                start_time: earliest,
                            });
                            last_reservation_end = earliest + duration;
                            last_gap = i;
                        } else {
                            if let Some(latest) = alternative.parameters.start_time.latest_start {
                                if last_reservation_end >= latest {
                                    // TODO(back track)
                                    let mut formula = vec![];
                                    for j in last_gap..i {
                                        let v = var_list.get(&sched[j]).expect("File should be ");
                                        formula.push(Lit::from_var(*v, false));
                                    }
                                    learned_clauses.push(formula);
                                    println!("Timed out {}, {}", latest, last_reservation_end);
                                    ok = false;
                                } else {
                                    resource_schedule.push(Assignment {
                                        id: sched[i].clone(),
                                        start_time: last_reservation_end,
                                    });
                                    last_reservation_end += duration;
                                }
                            } else {
                                resource_schedule.push(Assignment {
                                    id: sched[i].clone(),
                                    start_time: last_reservation_end,
                                });
                                last_reservation_end += duration;
                            }
                        }
                    }
                }
            }

            for clause in learned_clauses {
                solver.add_clause(&clause);
            }

            if ok {
                time_window = final_schedule.iter().filter(|(_resource, assignment)| {
                    assignment.len() != 0
                }).map(|(_resource, assignment)| {
                    let assignment = &assignment[assignment.len()-1];
                    assignment.start_time
                }).max();
            } else {
                println!("Could not solve");
            }
        }
    }

    pub fn feasbility_analysis(
        &self,
        problem: &Problem,
        stop: std::sync::Arc<AtomicBool>,
    ) -> Result<HashMap<String, Vec<Assignment>>, FlexibleSatError> {
        let mut resources = HashMap::new();
        let mut id_to_resource = vec![];
        let mut var_list = HashMap::new();
        let mut idx_to_option = vec![];

        let mut formula = varisat::CnfFormula::new();

        let mut var_by_resource = HashMap::new();

        let mut final_schedule = HashMap::new();

        for req_id in 0..problem.requests.len() {
            let mut options = vec![];
            let request_alternatives = &problem.requests[req_id];
            for alt_id in 0..request_alternatives.len() {
                let request = &request_alternatives[alt_id];
                if !resources.contains_key(&request.parameters.resource_name) {
                    resources.insert(
                        request.parameters.resource_name.clone(),
                        id_to_resource.len(),
                    );
                    var_by_resource.insert(id_to_resource.len(), vec![]);
                    id_to_resource.push(request.parameters.resource_name.clone());
                }
                let v = Var::from_index(idx_to_option.len());
                idx_to_option.push((req_id, alt_id));
                var_list.insert((req_id, alt_id), v);

                //NOTE: if this line panics something is  v weird. TODO(arjoc) reformat so impossible topanic.
                let mut option_list = var_by_resource
                    .get_mut(resources.get(&request.parameters.resource_name).unwrap());
                let Some(varlist) = option_list else {
                    panic!("We shouldnt reach here");
                };
                varlist.push((req_id, alt_id));
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

        let mut idx = idx_to_option.len();
        let mut comes_after_vars = HashMap::new();

        let mut idx_to_order = HashMap::new();
        // Strict total order variables
        for (_, alternatives) in var_by_resource.iter() {
            for i in 0..alternatives.len() {
                for j in 0..alternatives.len() {
                    if i == j {
                        continue;
                    }

                    let v = Var::from_index(idx);
                    idx_to_order.insert(idx, (alternatives[i], alternatives[j]));
                    idx += 1;

                    if !comes_after_vars.contains_key(&alternatives[i]) {
                        comes_after_vars.insert(alternatives[i], HashMap::new());
                    }
                    let Some(m) = comes_after_vars.get_mut(&alternatives[i]) else {
                        panic!("Should never reach here");
                    };
                    m.insert(alternatives[j], v);
                }
            }
        }

        // Strict Total Order constraints
        for (_, alternatives) in var_by_resource.iter() {
            for i in 0..alternatives.len() {
                for j in i + 1..alternatives.len() {
                    let ij = alternatives[i];
                    let km = alternatives[j];
                    let X_ijkm = comes_after_vars
                        .get(&ij)
                        .unwrap()
                        .get(&km)
                        .expect("something went wrong");
                    let X_kmij = comes_after_vars
                        .get(&km)
                        .unwrap()
                        .get(&ij)
                        .expect("something went wrong");
                    let x_ij = var_list.get(&ij).expect("Something went wrong");
                    let x_km = var_list.get(&km).expect("Something went wrong");

                    // Assymmetry
                    formula.add_clause(&[
                        Lit::from_var(*x_ij, false),
                        Lit::from_var(*x_km, false),
                        Lit::from_var(*X_ijkm, false),
                        Lit::from_var(*X_kmij, false),
                    ]);

                    // Connectedness
                    formula.add_clause(&[
                        Lit::from_var(*x_ij, false),
                        Lit::from_var(*x_km, false),
                        Lit::from_var(*X_ijkm, true),
                        Lit::from_var(*X_kmij, true),
                    ])
                }
            }

            // Transitivity (Warning O(n^3))
            for (_ij, x_ij_) in comes_after_vars.iter() {
                for (km, X_ijkm) in x_ij_.iter() {
                    let Some(other) = comes_after_vars.get(km) else {
                        continue;
                    };
                    for (nl, X_kmnl) in other.iter() {
                        let Some(X_ijnl) = x_ij_.get(nl) else {
                            //panic!("Failed to get {:?}", nl);
                            continue;
                        };

                        formula.add_clause(&[
                            Lit::from_var(*X_ijkm, false),
                            Lit::from_var(*X_kmnl, false),
                            Lit::from_var(*X_ijnl, true),
                        ]);
                    }
                }
            }
        }

        // Prededuced constraints based on scheduling constraints
        for (_, alternatives) in var_by_resource.iter() {
            for i in 0..alternatives.len() {
                for j in i + 1..alternatives.len() {
                    let alt_ij = alternatives[i];
                    let alt_km = alternatives[j];

                    let alt_ij_original = &problem.requests[alt_ij.0][alt_ij.1];
                    let alt_km_original = &problem.requests[alt_km.0][alt_km.1];

                    let Some(list_ij) = comes_after_vars.get(&alt_ij) else {
                        panic!("For some reason");
                    };

                    let X_ijkm = list_ij.get(&alt_km).expect("");
                    let Some(list_km) = comes_after_vars.get(&alt_km) else {
                        panic!("For some reason");
                    };

                    let X_kmij = list_km.get(&alt_ij).expect("");
                    if !alt_ij_original.can_be_scheduled_after(&alt_km_original.parameters) {
                        // ij cannot be after km
                        formula.add_clause(&[Lit::from_var(*X_ijkm, false)]);
                    }

                    if !alt_km_original.can_be_scheduled_after(&alt_ij_original.parameters) {
                        // ij cannot be after km
                        formula.add_clause(&[Lit::from_var(*X_kmij, false)]);
                    }
                }
            }
        }

        let mut solver = Solver::new();
        solver.add_formula(&formula);

        let mut solved = false;

        let current_time = self.clock_source.now();

        while !solved {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(FlexibleSatError::TimedOut);
            }

            final_schedule.clear();

            solver.solve();

            let Ok(k) = solver.solve() else {
                println!("Failed to solve");
                break;
            };

            if !k {
                println!("No soln");
                break;
            }

            let Some(model) = solver.model() else {
                break;
            };

            let mut edges = vec![];
            let mut vertices = vec![];
            for lit in model {
                if !lit.is_positive() {
                    continue;
                }
                let v = lit.var();
                let v_idx = v.index();

                if let Some((from, to)) = idx_to_order.get(&v_idx) {
                    edges.push(((*from), (*to)));
                } else {
                    if v_idx >= idx_to_option.len() {
                        continue;
                    }

                    let vert = idx_to_option[v_idx];
                    vertices.push(vert)
                }
            }

            // Build dependency graph
            let mut pgraph = Graph::<(usize, usize), bool>::new();
            let mut node_map = HashMap::new();

            for v in vertices {
                node_map.insert(v, pgraph.add_node(v));
            }
            for (after, before) in edges {
                pgraph.add_edge(
                    *node_map.get(&after).unwrap(),
                    *node_map.get(&before).unwrap(),
                    true,
                );
            }
            let Ok(res) = toposort(&pgraph, None) else {
                panic!("Sometthing wrong with SAT formula found cycle.");
            };
            let order: Vec<_> = res
                .iter()
                .map(|v| pgraph.raw_nodes()[v.index()].weight)
                .collect();
            let mut schedules: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

            for res_pair in order {
                let resource = &problem.requests[res_pair.0][res_pair.1]
                    .parameters
                    .resource_name;

                if let Some(sched) = schedules.get_mut(resource) {
                    sched.push(res_pair);
                } else {
                    schedules.insert(resource.clone(), vec![res_pair]);
                }
            }

            //println!("Schedule: {:?}", schedules);

            let mut learned_clauses = vec![];
            let mut ok = true;

            // Solve time slots. Can be parallelized.
            for (res_name, sched) in schedules {
                let mut last_reservation_end = current_time;
                let mut last_gap = 0usize;
                final_schedule.insert(res_name.clone(), vec![]);
                let Some(resource_schedule) = final_schedule.get_mut(&res_name) else {
                    panic!("Should never reach here")
                };
                for i in 0..sched.len() {
                    let alternative = &problem.requests[sched[i].0][sched[i].1];

                    let Some(duration) = alternative.parameters.duration else {
                        if i + 1 == sched.len() {
                            if let Some(latest) = alternative.parameters.start_time.latest_start {
                                if last_reservation_end > latest {
                                    // TODO(back track)
                                    println!("Timed out {}, {}", latest, last_reservation_end);
                                    let mut formula = vec![];
                                    for j in last_gap..i {
                                        let v = var_list
                                            .get(&sched[j])
                                            .expect("Could not get reservation end");
                                        formula.push(Lit::from_var(*v, false));
                                    }
                                    learned_clauses.push(formula);
                                    ok = false;
                                }
                            }
                            continue;
                        } else {
                            panic!("Somehow ended up with an infinite reservation with no end")
                        }
                    };

                    if let Some(earliest) = alternative.parameters.start_time.earliest_start {
                        if earliest >= last_reservation_end {
                            resource_schedule.push(Assignment {
                                id: sched[i].clone(),
                                start_time: earliest,
                            });
                            last_reservation_end = earliest + duration;
                            last_gap = i;
                        } else {
                            if let Some(latest) = alternative.parameters.start_time.latest_start {
                                if last_reservation_end >= latest {
                                    // TODO(back track)
                                    let mut formula = vec![];
                                    for j in last_gap..i {
                                        let v = var_list.get(&sched[j]).expect("File should be ");
                                        formula.push(Lit::from_var(*v, false));
                                    }
                                    learned_clauses.push(formula);
                                    println!("Timed out {}, {}", latest, last_reservation_end);
                                    ok = false;
                                } else {
                                    resource_schedule.push(Assignment {
                                        id: sched[i].clone(),
                                        start_time: last_reservation_end,
                                    });
                                    last_reservation_end += duration;
                                }
                            } else {
                                resource_schedule.push(Assignment {
                                    id: sched[i].clone(),
                                    start_time: last_reservation_end,
                                });
                                last_reservation_end += duration;
                            }
                        }
                    }
                }
            }

            for clause in learned_clauses {
                solver.add_clause(&clause);
            }

            if ok {
                solved = true;
            } else {
                println!("Could not solve");
            }
        }

        if solved {
            return Ok(final_schedule);
        } else {
            return Err(FlexibleSatError::NoSolution);
        }
    }
}

#[cfg(test)]
#[test]
fn test_flexible_one_item_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

    let current_time = chrono::Utc::now();

    let req1 = vec![ReservationRequest {
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

    let problem = Problem {
        requests: vec![req1],
    };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasbility_analysis(&problem, stop);
    let result = model.unwrap();

    assert_eq!(result.len(), 1usize);
    assert_eq!(result[&"Resource1".to_string()].len(), 1usize);
}

#[cfg(test)]
#[test]
fn test_flexible_two_items_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

    let current_time = chrono::Utc::now();

    let req1 = vec![ReservationRequest {
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
        ReservationRequest {
            parameters: crate::ReservationParameters {
                resource_name: "Resource1".to_string(),
                duration: Some(chrono::Duration::seconds(100)),
                start_time: crate::StartTimeRange {
                    earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                    latest_start: Some(current_time + chrono::Duration::seconds(120)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        },
        ReservationRequest {
            parameters: crate::ReservationParameters {
                resource_name: "Resource1".to_string(),
                duration: Some(chrono::Duration::seconds(100)),
                start_time: crate::StartTimeRange {
                    earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                    latest_start: Some(current_time + chrono::Duration::seconds(180)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        },
    ];

    let problem = Problem {
        requests: vec![req1, req2],
    };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasbility_analysis(&problem, stop);
    let result = model.unwrap();

    assert_eq!(result.len(), 1usize);
    assert_eq!(result[&"Resource1".to_string()].len(), 2usize);
    assert!(check_consistency(
        &result[&"Resource1".to_string()],
        &problem
    ))
}

#[cfg(test)]
#[test]
fn test_flexible_n_items_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

    let current_time = chrono::Utc::now();

    let n = 40usize;
    let task_dur = Duration::seconds(100);
    let mut requests = vec![];
    for i in 0..n {
        requests.push(vec![ReservationRequest {
            parameters: crate::ReservationParameters {
                resource_name: "Resource1".to_string(),
                duration: Some(task_dur),
                start_time: crate::StartTimeRange {
                    earliest_start: Some(current_time),
                    latest_start: Some(current_time + task_dur * (n as i32 + 1)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        }]);
    }

    let problem = Problem { requests };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasbility_analysis(&problem, stop);
    let result = model.unwrap();

    assert_eq!(result.len(), 1usize);
    assert_eq!(result[&"Resource1".to_string()].len(), n);
    assert!(check_consistency(
        &result[&"Resource1".to_string()],
        &problem
    ))
}

#[cfg(test)]
#[test]
fn test_flexible_no_soln_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

    let current_time = chrono::Utc::now();

    let n = 60usize;
    let task_dur = Duration::seconds(100);
    let mut requests = vec![];
    for i in 0..n {
        requests.push(vec![ReservationRequest {
            parameters: crate::ReservationParameters {
                resource_name: "Resource1".to_string(),
                duration: Some(task_dur),
                start_time: crate::StartTimeRange {
                    earliest_start: Some(current_time),
                    latest_start: Some(current_time + task_dur * (n as i32 + 1)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        }]);
    }

    let problem = Problem { requests };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasbility_analysis(&problem, stop);
    let result = model.unwrap();

    assert_eq!(result.len(), 1usize);
    assert_eq!(result[&"Resource1".to_string()].len(), n);
    assert!(check_consistency(
        &result[&"Resource1".to_string()],
        &problem
    ))
}

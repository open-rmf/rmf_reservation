use std::{
    collections::{HashMap, HashSet, VecDeque}, sync::{atomic::AtomicBool, mpsc::Sender, Arc}
};

use itertools::Itertools;
use petgraph::{algo::{find_negative_cycle, toposort}, Graph};
use test::filter_tests;
use varisat::{CnfFormula, ExtendFormula, Lit, Solver, Var};

use chrono::{prelude::*, Duration, TimeDelta};

use crate::{cost_function::static_cost::{self, StaticCost}, database::ClockSource};
use crate::ReservationRequestAlternative;

use super::{AlgorithmState, SolverAlgorithm};

/// Snapshot of requests that need to be solved.
///
/// This is how you specify a problem set that needs solving.
#[derive(Debug, Clone, Default)]
pub struct Problem {
    /// A vector of requests. A solved problem will satisfy at least one "alternative"
    /// within a request.
    pub requests: Vec<Vec<ReservationRequestAlternative>>,

    /// Dependency one of the
    pub one_of_dependencies: Vec<Vec<(usize, usize)>>,

    /// Dependency of the form (a, b) where a is the request id and b is the alternative id.
    pub dependencies: Vec<((usize, usize), (usize, usize))>,

    /// Must be immediately after constraint
    /// of the form (a, b) where a is the request id and b is the alternative id.
    pub must_be_immediately_after: Vec<((usize, usize), (usize, usize))>,

    /// Dependency of the form (a, b) where a is the request id and b is the alternative id.
    /// (a,b)
    pub same_start: HashMap<(usize, usize), (usize, usize)>,
}

impl Problem {
    /// Request one alternative out of a few
    /// Returns the index of the request. This is useful for checking the assignment later on.
    pub fn request_one_of(&mut self, alternatives: Vec<ReservationRequestAlternative>) -> usize {
        self.requests.push(alternatives);
        return self.requests.len() - 1;
    }

    /// Request one or no alternative out of a few
    /// Returns the index of the request. This is useful for checking the assignment later on.
    pub fn request_one_of_or_none(&mut self, alternatives: Vec<ReservationRequestAlternative>) -> usize {
        let mut alternatives = alternatives.clone();
        // TODO(arjoc): 
        alternatives.push(
            ReservationRequestAlternative {
                parameters: crate::ReservationParameters { 
                    resource_name: "".to_string(), 
                    duration: TimeDelta::new(0, 0), 
                    start_time: crate::StartTimeRange { 
                        earliest_start: None, 
                        latest_start: None 
                    }
                },
                cost_function: Arc::new(StaticCost::new(0f64))
            }
        );
        self.requests.push(alternatives);
        return self.requests.len() - 1;
    }

    pub fn implies(&mut self, a: &(usize, usize), b: &(usize, usize)) -> Result<(), String> {
        if a.0 >= self.requests.len()
            || a.1 >= self.requests[a.0].len()
            || b.0 >= self.requests.len()
            || b.1 >= self.requests[b.0].len()
        {
            return Err("Request and alternative was not found.".to_string());
        }
        self.dependencies.push((*a, *b));
        Ok(())
    }

    /// Require that these must a come immediately after b, if a and b are both awarded.
    /// For now we don't support cross-resource request.
    pub fn must_come_immediately_after(&mut self,
        a: &(usize, usize), b: &(usize, usize)) -> Result<(), String> {
        if a.0 >= self.requests.len()
            || a.1 >= self.requests[a.0].len()
            || b.0 >= self.requests.len()
            || b.1 >= self.requests[b.0].len()
            || self.requests[b.0][b.1].parameters.resource_name != self.requests[a.0][a.1].parameters.resource_name
        {
            return Err("Request and alternative was not found or between two different resources".to_string());
        }
        self.must_be_immediately_after.push((*a, *b));

        Ok(())
    }

    /// Require that a and b start at the same time.
    /// WARNING: We do not yet support transitivity.
    pub fn must_start_at_same_time(&mut self,
        a: &(usize, usize), b: &(usize, usize)) -> Result<(), String> {
        if a.0 >= self.requests.len()
            || a.1 >= self.requests[a.0].len()
            || b.0 >= self.requests.len()
            || b.1 >= self.requests[b.0].len()
            || self.requests[b.0][b.1].parameters.resource_name == self.requests[a.0][a.1].parameters.resource_name
        {
            return Err("Request and alternative was not found or between two different resources".to_string());
        }
        self.same_start.insert(*a, *b);
        self.same_start.insert(*b, *a);
        Ok(())
    }

}

/// Snapshot of a solution. A solved schedule contains a list of assingments for each resource
#[derive(Debug, Clone)]
pub struct Assignment {
    /// For a given solution this refers to the alternative.
    /// The first index refers to the request ID you retrieved from `request_one_of` or
    /// the index of the alternatives. The second index refers to which alternative/resource
    /// needs to be used.
    pub id: (usize, usize),

    /// Start time of a said assignment.
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

fn shrink_reservation_request(
    reservation_req: &ReservationRequestAlternative,
    time_window: DateTime<Utc>,
) -> Option<ReservationRequestAlternative> {
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

    Some(ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: reservation_req.parameters.resource_name.clone(),
            duration: reservation_req.parameters.duration.clone(),
            start_time: crate::StartTimeRange {
                earliest_start: reservation_req.parameters.start_time.earliest_start.clone(),
                latest_start: Some(time_window),
            },
        },
        cost_function: reservation_req.cost_function.clone(),
    })
}


/// Solves the same time constraint
fn solve_same_time_constraints(final_schedule: &HashMap<String, Vec<Assignment>>, problem: &Problem) ->Result<HashMap<String, Vec<Assignment>>, ()> {
    
    let mut final_schedule = final_schedule.clone();
    let mut indices = HashMap::new();
    let mut start_times_and_resources = HashMap::new();
    let mut earliest_time = None;
    for (resource, schedule) in &final_schedule {
        let Some(assignment) = schedule.first() else {
            continue;
        };
        if earliest_time == None {
            earliest_time = Some(assignment.start_time);
        }
        else {
            earliest_time = Some(earliest_time.unwrap().min(assignment.start_time));
        }
        indices.insert(resource.clone(), 0usize);

        for (index, assignment) in schedule.iter().enumerate() {
            start_times_and_resources.insert(assignment.id, (resource.clone(), index));
        }
    }
    let mut delay_graph = HashMap::new();
    let mut visited = HashSet::new();
    println!("{:?}", problem.same_start);
    let mut last_delay = problem.same_start.iter()
        .map(|(u1, u2)| 
        {

            let (res1, idx1) = start_times_and_resources[&u1].clone();
            let (res2, idx2) = start_times_and_resources[&u2].clone();
            let start1 = final_schedule[&res1][idx1].start_time.clone();
            let start2 = final_schedule[&res2][idx2].start_time.clone();
            println!("{:?} {:?}",start1, start2);
            if start1 < start2 {
                (u2, start2, u1)
            }
            else {
                (u1, start1, u2)
            }
        })
        .fold(None, |a, b| {
            if let Some((delay_cause, start_time, delay_affected)) = a {
                if b.1 > start_time {
                    Some(b)
                }
                else {
                    a
                }
            }
            else {
                Some(b)
            }
        });
    println!("Last Delay {:?}", last_delay);
    while let Some((delay_cause, start_time, delay_affected)) = last_delay {
        visited.insert((delay_cause, delay_affected));
        visited.insert((delay_affected, delay_cause));
        // For backtracking
        delay_graph.insert(*delay_affected, *delay_cause);
        let (resource, affected_id) = start_times_and_resources[&delay_affected].clone();
        
        // Attempt to delay the resource
        let Some(resource_sched) = final_schedule.get_mut(&resource) else {
            panic!();
        };
        resource_sched[affected_id].start_time = start_time;
        for (i,j) in (affected_id..resource_sched.len()).tuple_windows() {
            let delay_affected = resource_sched[i].id;
            let next_in_line = resource_sched[j].id;
            let Some(p) = problem.requests[delay_affected.0][delay_affected.1].parameters.duration else {
                panic!("Schedule had indeterminate duration at end");
            };
            if resource_sched[j].start_time > resource_sched[i].start_time + p {
                break;
            }
            
            resource_sched[j].start_time = resource_sched[i].start_time + p;
            delay_graph.insert(next_in_line, delay_affected);
        }

        last_delay = problem.same_start.iter()
        .filter(|edge| !visited.contains(edge))
        .map(|(u1, u2)| 
        {

            let (res1, idx1) = start_times_and_resources[&u1].clone();
            let (res2, idx2) = start_times_and_resources[&u2].clone();
            let start1 = final_schedule[&res1][idx1].start_time.clone();
            let start2 = final_schedule[&res2][idx2].start_time.clone();
            println!("{:?} {:?}",start1, start2);
            if start1 < start2 {
                (u2, start2, u1)
            }
            else {
                (u1, start1, u2)
            }
        })
        .fold(None, |a, b| {
            if let Some((delay_cause, start_time, delay_affected)) = a {
                if b.1 > start_time {
                    Some(b)
                }
                else {
                    a
                }
            }
            else {
                Some(b)
            }
        });
    }
    Ok(final_schedule)
}

#[test]
fn test_solve_time_constraints()
{
    let current_time = chrono::Utc::now();
    let mut problem = Problem::default();

    let req1 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource1".to_string(),
            duration: Some(chrono::Duration::seconds(60)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(120)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];
    let w0 = problem.request_one_of(req1);
    let req2 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource1".to_string(),
            duration: Some(chrono::Duration::seconds(60)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(120)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];
    let w1 = problem.request_one_of(req2);

    let req3 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource2".to_string(),
            duration: Some(chrono::Duration::seconds(60)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(120)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];
    let w2 = problem.request_one_of(req3);

    problem.must_start_at_same_time(&(w0, 0), &(w2, 0));

    let mut final_schedule = HashMap::new();
    final_schedule.insert("Resource2".to_string(), vec![
        Assignment {
            id: (w2, 0),
            start_time: current_time + chrono::Duration::seconds(50)
        }
    ]);
    final_schedule.insert("Resource1".to_string(), vec![
        Assignment {
            id: (w0, 0),
            start_time: current_time 
        },
        Assignment {
            id: (w1, 0),
            start_time: current_time + chrono::Duration::seconds(60)
        },
    ]);

    let Ok(sched) = solve_same_time_constraints(&final_schedule, &problem) else
    {
        panic!("Got error instead of solution");
    };
    println!("{:?}", sched);
    assert_eq!(sched["Resource2"][0].start_time, sched["Resource1"][0].start_time);
    assert!(sched["Resource1"][1].start_time > sched["Resource1"][0].start_time);
}

/// Solver for scenarios where there is a starting time range instead of a fixed starting time.
/// Note: The solvers in this class currently ignore the cost of an alternative.
/// You need to implement a clock source. The reason is that we needto have the current time as a starting point.
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
        let Ok(feasible_solution) = self.feasibility_analysis(&problem, stop.clone()) else {
            result_channel.send(AlgorithmState::UnSolveable);
            return;
        };

        result_channel.send(AlgorithmState::FeasibleScheduleSolution(feasible_solution));

        self.time_optimality_solver(&problem, result_channel, stop);
    }
}

impl<CS: ClockSource + Clone + std::marker::Send + std::marker::Sync> SATFlexibleTimeModel<CS> {
    /// This class of solvers tries to pack all the alternatives into the shortest possible time window
    /// It ignores the cost function. This is useful if you want to pack more items
    /// - `problem` - A reservation problem you want to solve.
    /// - `sender` - A channel by which the solver communicates its latest "best" solution. This is useful
    /// for scenarios where the solver is taking too long and you need a feasible solution soon. You can listen on this
    /// channel without calling `feasbility_analysis`.
    /// - `stop` - A boolean by which you can tell the solver to stop solving.
    pub fn time_optimality_solver(
        &self,
        problem: &Problem,
        sender: Sender<AlgorithmState>,
        stop: std::sync::Arc<AtomicBool>,
    ) {
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

        /// Dependency requirements
        for dep in problem.dependencies.iter() {
            let (x1, x2) = dep;
            let Some(x_ij) = var_list.get(x1) else {
                panic!("Could not find variable");
            };
            let Some(x_km) = var_list.get(x2) else {
                panic!("Could not get variable");
            };
            formula.add_clause(&[x_ij.negative(), Lit::from_var(*x_km, true)]);
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

        // Constraints for coming immediately after.
        // If a and b are awarded and in the same resource, then b must come immediately after a
        // and nothing else.
        for (ij, km) in problem.must_be_immediately_after.iter() {
            if problem.requests[ij.0][ij.1].parameters.resource_name != problem.requests[km.0][km.1].parameters.resource_name {
                continue;
            }
            let Some(x_ij) = var_list.get(&ij) else {
                panic!("Could not find variable");
            };
            let Some(x_km) = var_list.get(&km) else {
                panic!("Could not get variable");
            };
            let Some(x_ij_) = comes_after_vars.get(ij) else
            {
                continue;
            };

            let mut sum_vars = vec![];
            for (nl,x_ijnl) in x_ij_ {
                if nl == km {
                    formula.add_clause(&[
                        Lit::from_var(*x_ij, false),
                        Lit::from_var(*x_km, false),
                        Lit::from_var(*x_ijnl, true)
                    ]);
                }
                else {
                    sum_vars.push(Lit::from_var(*x_ijnl, true));
                }
            }
            formula.add_clause(&sum_vars);
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
        let mut prev_schedule = HashMap::new();

        while !solved {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                sender.send(AlgorithmState::NotFound);
                return;
            }

            prev_schedule = final_schedule;
            final_schedule = HashMap::new();

            // Shrink the time window. Recalculate
            if let Some(time_window) = time_window {
                println!("Attempting shrink");
                let mut formula = varisat::CnfFormula::new();
                for (_, alternatives) in var_by_resource.iter() {
                    for i in 0..alternatives.len() {
                        for j in i + 1..alternatives.len() {
                            let alt_ij = alternatives[i];
                            let alt_km = alternatives[j];

                            let alt_ij_shrink = shrink_reservation_request(
                                &problem.requests[alt_ij.0][alt_ij.1],
                                time_window,
                            );
                            let alt_km_shrink = shrink_reservation_request(
                                &problem.requests[alt_km.0][alt_km.1],
                                time_window,
                            );

                            println!("Shrinking to");
                            println!("{:?}", alt_ij_shrink);
                            println!("{:?}", alt_km_shrink);

                            if alt_ij_shrink.is_none() {
                                // Ban the entire alternative

                                let x_ij = var_list.get(&alt_ij).expect("Something went wrong");
                                formula.add_clause(&[Lit::from_var(*x_ij, false)]);
                            }

                            if alt_km_shrink.is_none() {
                                // Ban the entire alternative
                                let x_km = var_list.get(&alt_km).expect("Something went wrong");
                                formula.add_clause(&[Lit::from_var(*x_km, false)]);
                            }

                            if let Some(alt_ij_shrink) = alt_ij_shrink {
                                if let Some(alt_km_shrink) = alt_km_shrink {
                                    let Some(list_ij) = comes_after_vars.get(&alt_ij) else {
                                        panic!("For some reason unable to get comes after vars");
                                    };

                                    let X_ijkm = list_ij.get(&alt_km).expect("");
                                    let Some(list_km) = comes_after_vars.get(&alt_km) else {
                                        panic!(
                                            "For some reason unable to get comes after vars
                                        "
                                        );
                                    };

                                    let X_kmij = list_km.get(&alt_ij).expect("");
                                    if !alt_ij_shrink
                                        .can_be_scheduled_after(&alt_km_shrink.parameters)
                                    {
                                        // ij cannot be after km
                                        formula.add_clause(&[Lit::from_var(*X_ijkm, false)]);
                                    }

                                    if !alt_km_shrink
                                        .can_be_scheduled_after(&alt_ij_shrink.parameters)
                                    {
                                        // ij cannot be after km
                                        formula.add_clause(&[Lit::from_var(*X_kmij, false)]);
                                    }
                                }
                            }
                        }
                    }
                }
                solver.add_formula(&formula);
            }

            println!("Solving");

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

            println!("Reconstructing proposed schedule");

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
                panic!("Something wrong with SAT formula found cycle.");
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

            // Solve time slots without cross-resource time constraints. Can be parallelized.
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
                        if i + 1 < sched.len() {
                            if let Some(latest) = alternative.parameters.start_time.latest_start {
                                if last_reservation_end > latest {
                                    // Add a banning of this specific ordering [Exponential bomb if ordering is too long]
                                    for (j, k) in (last_gap..i).tuple_windows() {
                                        let j_id = sched[j];
                                        let Some(vars) = comes_after_vars.get(&j_id) else {
                                            continue;
                                        };

                                        let mut transitive_pairs = vec![];

                                        for (id, j_var) in vars.iter() {
                                            if *id == sched[j] || *id == sched[k] {
                                                continue;
                                            }
                                            if problem.requests[id.0][id.1].parameters.resource_name
                                                != problem.requests[j_id.0][j_id.1]
                                                    .parameters
                                                    .resource_name
                                            {
                                                continue;
                                            }
                                            let Some(other) = comes_after_vars.get(&id) else {
                                                panic!("Could not get");
                                            };
                                            let Some(k_var) = other.get(&sched[k]) else {
                                                continue;
                                            };
                                            transitive_pairs.push((j_var, k_var));
                                        }

                                        if transitive_pairs.len() > 12 {
                                            panic!("Problem is too congested to solve");
                                        }

                                        let Some(not_allowed_next) = vars.get(&sched[k]) else {
                                            continue;
                                        };

                                        for x in 0..2_i32.pow(transitive_pairs.len() as u32) {
                                            let mut clause = vec![];
                                            for y in 0..transitive_pairs.len() {
                                                if (1 << y) & x != 0 {
                                                    clause.push(transitive_pairs[y].0);
                                                } else {
                                                    clause.push(transitive_pairs[y].1);
                                                }
                                            }

                                            let mut formula =
                                                vec![Lit::from_var(*not_allowed_next, false)];
                                            for i in clause {
                                                formula.push(Lit::from_var(*i, true));
                                            }
                                            learned_clauses.push(formula);
                                        }
                                    }
                                } else {
                                    if last_reservation_end != latest {
                                        last_gap = i;
                                    }
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

            
            
            // Solve time slots with cross-resource time constraints.
           
            
            println!("learned_clauses {:?}", learned_clauses.len());
            if learned_clauses.len() == 0 {
                sender.send(AlgorithmState::FeasibleScheduleSolution(
                    final_schedule.clone(),
                ));
            }

            for clause in learned_clauses {
                solver.add_clause(&clause);
            }

            if ok {
                println!("{:?}", final_schedule);
                time_window = final_schedule
                    .iter()
                    .filter(|(_resource, assignment)| assignment.len() != 0)
                    .map(|(_resource, assignment)| {
                        let assignment = &assignment[assignment.len() - 1];
                        assignment.start_time
                    })
                    .max();
                println!("Setting new time window to be less than {:?}", time_window);
                // We also don't want the same solution
                let banned_assignment: Vec<_> = final_schedule
                    .iter()
                    .map(|(_, assignment)| assignment.iter())
                    .flatten()
                    .map(|p| Lit::from_var(var_list[&p.id], false))
                    .collect();
                solver.add_clause(&banned_assignment);
            } else {
                println!("Could not solve");
                ok = false;
            }
        }
        sender.send(AlgorithmState::OptimalScheduleSolution(
            prev_schedule.clone(),
        ));
    }

    /// Checks if a set of requests is feasible. Given a problem we see if there is a way to schedule the solution while ignoring the
    /// cost function.
    /// - `problem` - Takes in th eproblem that needs to be optimized.
    /// - `stop` - Takes in an atomic boolean that allows you to stop the computation from another thread.
    /// Returns a result containing an example feasible schedule if OK. Otherwise, returns an error.
    /// The feasible schedule is a HashMap where the key of the hashmap is the resource and thevalue of the hashmap is the assigned alternative.
    pub fn feasibility_analysis(
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
fn test_multi_item_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

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

    let req2 = vec![ReservationRequestAlternative {
        parameters: crate::ReservationParameters {
            resource_name: "Resource1".to_string(),
            duration: Some(chrono::Duration::seconds(100)),
            start_time: crate::StartTimeRange {
                earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                latest_start: Some(current_time + chrono::Duration::seconds(160)),
            },
        },
        cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
    }];

    let mut problem = Problem::default();
    problem.request_one_of(req1);
    problem.request_one_of(req2);

    let (sender, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .time_optimality_solver(&problem, sender, stop);
    for t in rx.iter() {
        println!("{:?}", t)
    }
}

#[cfg(test)]
#[test]
fn test_multi_alternative_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

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

    let (sender, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .time_optimality_solver(&problem, sender, stop);
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
    assert_eq!(sched.len(), 2usize);
}

#[cfg(test)]
#[test]
fn test_multi_alternative_sat_solver_with_dep() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

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
    let req1_id = problem.request_one_of(req1);
    let req2_id = problem.request_one_of(req2);

    problem.implies(&(req1_id, 0), &(req2_id, 0));
    problem.implies(&(req2_id, 0), &(req1_id, 0));

    let (sender, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .time_optimality_solver(&problem, sender, stop);

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
    assert_eq!(sched["Resource1"].len(), 2usize);
    assert_eq!(sched["Resource1"][1].id, (1usize, 0usize));
    assert_eq!(sched.len(), 1usize);
}

#[cfg(test)]
#[test]
fn test_flexible_one_item_sat_solver() {
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

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

    let problem = Problem {
        requests: vec![req1],
        ..Default::default()
    };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasibility_analysis(&problem, stop);
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
                    earliest_start: Some(current_time + chrono::Duration::seconds(50)),
                    latest_start: Some(current_time + chrono::Duration::seconds(120)),
                },
            },
            cost_function: Arc::new(static_cost::StaticCost::new(1.0)),
        },
        ReservationRequestAlternative {
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
        ..Default::default()
    };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasibility_analysis(&problem, stop);
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
    use std::default;
    use std::sync::Arc;

    use crate::cost_function::static_cost;

    use crate::database::DefaultUtcClock;

    let current_time = chrono::Utc::now();

    let n = 40usize;
    let task_dur = Duration::seconds(100);
    let mut requests = vec![];
    for i in 0..n {
        requests.push(vec![ReservationRequestAlternative {
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

    let problem = Problem {
        requests,
        dependencies: vec![],
        one_of_dependencies: vec![],
        ..Default::default()
    };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasibility_analysis(&problem, stop);
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
        requests.push(vec![ReservationRequestAlternative {
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

    let problem = Problem {
        requests,
        ..Default::default()
    };

    let stop = Arc::new(AtomicBool::new(false));
    let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .feasibility_analysis(&problem, stop);
    let result = model.unwrap();

    assert_eq!(result.len(), 1usize);
    assert_eq!(result[&"Resource1".to_string()].len(), n);
    assert!(check_consistency(
        &result[&"Resource1".to_string()],
        &problem
    ))
}

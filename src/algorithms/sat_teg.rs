use std::collections::{HashMap, HashSet};

use super::sat_flexible_time_model::Problem;

/// Solve resource scheduling problems with
/// time-expansions taken into mind.
struct TEGSolver {
    time_step: chrono::Duration,
    max_time_steps: chrono::Duration,
}

impl TEGSolver {
    fn solve(problem: Problem) {
        //let decision_variables = HashMap::new();
        let mut resources = HashMap::new();
        let mut idx_to_res = Vec::new();
        for r in problem.requests {
            for alt in r {
                resources.insert(alt.parameters.resource_name.clone(), idx_to_res.len());
                idx_to_res.push(alt.parameters.resource_name.clone());
            }
        }
    }
}

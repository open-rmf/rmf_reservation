use std::{collections::HashMap, sync::{Arc, Mutex}};

use self::greedy_solver::Solution;

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
    should_continue: bool
}

impl Default for AlgorithmPool {
    fn default() -> Self {
        Self { 
            proposed_solution: AlgorithmState::NotFound, 
            should_continue: Default::default()
        }
    }
}

impl AlgorithmPool {
    
    pub(crate) fn found_unsatisfiable(&mut self) {
        self.proposed_solution = AlgorithmState::UnSolveable;
    }

    pub(crate) fn optimal_solution(&mut self, solution: &HashMap<usize, usize>) {
        self.proposed_solution = AlgorithmState::OptimalSolution(solution.clone());
    }

    pub(crate) fn propose_solution(&mut self, solution: &HashMap<usize, usize>, cost: f64) {
        self.proposed_solution = AlgorithmState::PartialSolution(solution.clone(), cost);
    }

    fn should_continue(&self) -> bool {
        match self.proposed_solution {
            AlgorithmState::OptimalSolution(_) => false,
            AlgorithmState::PartialSolution(_, _) => true,
            AlgorithmState::NotFound => true,
            AlgorithmState::UnSolveable => false,
        }
    }
}


pub(crate) trait Algorithm {
    fn iterative_solve(&self, pool: &Arc<Mutex<AlgorithmPool>>);
}
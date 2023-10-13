use std::{collections::HashMap, default, hash::Hash};

use crate::{ReservationRequest, algorithms::greedy_solver::{ConflictTracker, Problem}};

pub struct Ticket {
    count: usize
}


enum ReservationState {
    Claimed(usize),
}
#[derive(Debug, Clone)]
struct Snapshot {
    snapshot_tracker: HashMap<usize, usize>,
    problem: Problem
}

#[derive(Default)]
pub struct FixedTimeReservationSystem {
    resources: Vec<String>,
    record: HashMap<usize, Vec<ReservationRequest>>,
    claims: HashMap<usize, ReservationState>,
    max_id: usize,
}

impl FixedTimeReservationSystem {

    pub fn create_with_resources(resources: Vec<String>) -> Self {
        Self {
            resources,
            ..Default::default()
        }
    }

    pub fn request_resources(&mut self, alternatives: Vec<ReservationRequest>) -> Result<Ticket, &'static str> {
        for alt in &alternatives {
            if alt.parameters.start_time.earliest_start != alt.parameters.start_time.latest_start &&
                alt.parameters.start_time.earliest_start.is_some() {
                return Err("FixedTimeReservationSystem supports only fixed time frames");
            }
        }
        self.record.insert(self.max_id, alternatives);
        let result = Ticket { count: self.max_id  };
        self.max_id += 1;

        Ok(result)
    }

    fn get_snapshot(&mut self) -> Snapshot {
        let mut conflict_tracker = ConflictTracker::create_with_resources(&self.resources);
        let mut snapshot_tracker = HashMap::new();
        
        for (ticket, record) in &self.record {
            if let Some(ReservationState::Claimed(index)) = self.claims.get(&ticket) {
                conflict_tracker.request_resources(vec![record[*index].clone()]);
                continue;
            };

            let Some(tracker) = conflict_tracker.request_resources(record.clone()) else {
                continue;
            };

            snapshot_tracker.insert(*ticket, tracker);
        }

        let problem = conflict_tracker.generate_literals_and_remap_requests();

        Snapshot {
            snapshot_tracker,
            problem
        }
    }
}
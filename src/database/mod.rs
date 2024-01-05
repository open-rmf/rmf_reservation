use std::{collections::HashMap, default, hash::Hash, sync::Arc, fs::Metadata};

use serde_derive::{Serialize, Deserialize};

use crate::{ReservationRequest, algorithms::{greedy_solver::{ConflictTracker, Problem, GreedySolver}, AsyncExecutor, AlgorithmPool, sat::SATSolver, sat_flexible_time_model::SATFlexibleTimeModel}, Assignment, StartTimeRange};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    count: usize
}

impl Ticket {
    pub fn get_id(&self) -> usize {
        self.count
    }

    pub fn from_id(id: usize) -> Self {
        Self { count: id }
    }
}

enum ReservationState {
    Claimed(usize),
}
#[derive(Debug, Clone)]
pub(crate) struct Snapshot<P, T = ()> {
    pub(crate) problem: P,
    pub(crate) metadata: T
}

pub struct FixedTimeReservationSystem {
    resources: Vec<String>,
    record: HashMap<usize, Vec<ReservationRequest>>,
    claims: HashMap<usize, ReservationState>,
    max_id: usize,
    async_executor: AsyncExecutor<Problem>
}

impl FixedTimeReservationSystem {

    pub fn create_with_resources(resources: Vec<String>) -> Self {
        
        let mut alg_pool = AlgorithmPool::<Problem>::default();
        alg_pool.add_algorithm(Arc::new(SATSolver));
        alg_pool.add_algorithm(Arc::new(GreedySolver));
        
        Self {
            resources,
            async_executor: AsyncExecutor::init(alg_pool),
            record: HashMap::new(),
            claims: HashMap::new(),
            max_id: 0,
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

        let snapshot = self.get_snapshot();
        self.async_executor.attempt_solve(snapshot);

        Ok(result)
    }

    pub fn claim_request(&mut self, ticket: Ticket) -> Option<usize> {
        let result = self.async_executor.retrieve_best_fixed_time_solution_and_stop();
        if let Some(res) = result {
            println!("{:?}", res);
            if let Some(res) = res.get(&ticket.count) {
                self.claims.insert(ticket.count, ReservationState::Claimed(*res));
                return Some(*res);
            }
        }
        None
    }

    fn get_snapshot(&mut self) -> Snapshot<Problem> {
        let mut conflict_tracker = ConflictTracker::create_with_resources(&self.resources);
        
        for (ticket, record) in &self.record {
            if let Some(ReservationState::Claimed(index)) = self.claims.get(&ticket) {
                conflict_tracker.request_resources(vec![record[*index].clone()]);
                continue;
            };
            conflict_tracker.request_resources(record.clone());
        }

        let problem = conflict_tracker.generate_literals_and_remap_requests();
        println!("{:?}", problem);
        Snapshot {
            problem, metadata: ()
        }
    }
}

#[derive(Default, Debug, Clone)]
struct FlexibleTimeReservationSystemMetadata {
    mapping: HashMap<usize, usize>
}

pub struct FlexibleTimeReservationSystem {
    resources: Vec<String>,
    record: HashMap<usize, Vec<ReservationRequest>>,
    claims: HashMap<usize, super::algorithms::sat_flexible_time_model::Assignment>,
    max_id: usize,
    async_executor: AsyncExecutor<super::algorithms::sat_flexible_time_model::Problem, FlexibleTimeReservationSystemMetadata>
}

impl FlexibleTimeReservationSystem {

    pub fn create_with_resources(resources: Vec<String>) -> Self {
        
        let mut alg_pool = AlgorithmPool::<super::algorithms::sat_flexible_time_model::Problem>::default();
        alg_pool.add_algorithm(Arc::new(SATFlexibleTimeModel));
        
        Self {
            resources,
            async_executor: AsyncExecutor::init(alg_pool),
            record: HashMap::new(),
            claims: HashMap::new(),
            max_id: 0,
        }
    }

    pub fn request_resources(&mut self, alternatives: Vec<ReservationRequest>) -> Result<Ticket, &'static str> {
        self.record.insert(self.max_id, alternatives);
        let result = Ticket { count: self.max_id  };
        self.max_id += 1;

        let snapshot = self.get_snapshot();
        self.async_executor.attempt_solve(snapshot);

        Ok(result)
    }

    pub fn claim_request(&mut self, ticket: Ticket) -> Option<usize> {
        let Some((result, Metadata)) = self.async_executor.retrieve_feasible_schedule() else {
            return None;
        };

        if let Some(res) = result {
            println!("{:?}", res);
            if let Some(res) = res.get(&ticket.count) {
                self.claims.insert(ticket.count, ReservationState::Claimed(*res));
                return Some(*res);
            }
        }
        None
    }

    pub fn extend_request(&mut self, ticket: &Ticket) -> Result<(), String>{
        Err(format!("Unimplimented"))
    }

    fn get_snapshot(&mut self) -> Snapshot<super::algorithms::sat_flexible_time_model::Problem, FlexibleTimeReservationSystemMetadata> {

        let mut requests = vec![];
        let mut mapping = HashMap::<usize, usize>::new();
        for (key, alts) in self.record {
            mapping.insert(requests.len(), key);
            if let Some(reservation_state) = self.claims.get(&key) {
                let mut selected = alts[reservation_state.id.1].clone();
                selected.parameters.start_time = StartTimeRange::exactly_at(&reservation_state.start_time);
                requests.push(vec![selected]);
            }
            else  {
                requests.push(alts.clone());
            }
        }

        let problem = super::algorithms::sat_flexible_time_model::Problem {
            requests
        };
        
        Snapshot {
            problem,
            metadata: FlexibleTimeReservationSystemMetadata {
                mapping
            }
        }
    }
}

#[cfg(test)]
#[test]
fn test_fixed_time() {
    use chrono::{Duration, Utc, TimeZone};

    use crate::{StartTimeRange, cost_function::static_cost};

    let resources = vec!["res1".to_string(), "res2".to_string()];
    let mut res_sys = FixedTimeReservationSystem::create_with_resources(resources);

    let alternatives = vec![
        ReservationRequest {
            parameters: crate::ReservationParameters{
                resource_name: "res1".to_string(),
                duration: Some(Duration::minutes(10)),
                start_time:StartTimeRange::exactly_at(&Utc.with_ymd_and_hms(2023,7,8,7,10,11).unwrap())
            }, 
            cost_function: Arc::new(static_cost::StaticCost::new(2.0)) }];

    let ticket = res_sys.request_resources(alternatives).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let claim = res_sys.claim_request(ticket).unwrap();
}

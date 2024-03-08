use std::{collections::HashMap, default, fs::Metadata, hash::Hash, sync::Arc};

use chrono::{DateTime, Duration, Utc};
use serde_derive::{Deserialize, Serialize};

use crate::{
    algorithms::{
        greedy_solver::{ConflictTracker, GreedySolver, Problem},
        sat::SATSolver,
        sat_flexible_time_model::{Assignment as FlexibleAssignment, SATFlexibleTimeModel},
        AlgorithmPool, AsyncExecutor,
    },
    wait_points::{
        self,
        wait_points::{WaitPointInfo, WaitPointRequest, WaitPointSystem},
    },
    ReservationRequest, StartTimeRange,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    count: usize,
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
    pub(crate) metadata: T,
}

pub struct FixedTimeReservationSystem {
    resources: Vec<String>,
    record: HashMap<usize, Vec<ReservationRequest>>,
    claims: HashMap<usize, ReservationState>,
    max_id: usize,
    async_executor: AsyncExecutor<Problem, ()>,
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

    pub fn request_resources(
        &mut self,
        alternatives: Vec<ReservationRequest>,
    ) -> Result<Ticket, &'static str> {
        for alt in &alternatives {
            if alt.parameters.start_time.earliest_start != alt.parameters.start_time.latest_start
                && alt.parameters.start_time.earliest_start.is_some()
            {
                return Err("FixedTimeReservationSystem supports only fixed time frames");
            }
        }
        self.record.insert(self.max_id, alternatives);
        let result = Ticket { count: self.max_id };
        self.max_id += 1;

        let snapshot = self.get_snapshot();
        self.async_executor.attempt_solve(snapshot);

        Ok(result)
    }

    pub fn claim_request(&mut self, ticket: Ticket) -> Option<usize> {
        let result = self
            .async_executor
            .retrieve_best_fixed_time_solution_and_stop();
        if let Some(res) = result {
            println!("{:?}", res);
            if let Some(res) = res.get(&ticket.count) {
                self.claims
                    .insert(ticket.count, ReservationState::Claimed(*res));
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
            problem,
            metadata: (),
        }
    }
}

pub trait ClockSource {
    fn now(&self) -> chrono::DateTime<Utc>;
}

#[derive(Default, Clone)]
pub struct DefaultUtcClock;

impl ClockSource for DefaultUtcClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        return Utc::now();
    }
}

#[derive(Debug, Clone)]
pub struct Goal {
    pub resource: String,
    pub satisfies_alt: usize,
    pub time: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum ClaimSpot {
    GoImmediately(Goal),
    WaitAtThenGo(usize, Goal),
    WaitPermanently(usize),
}

#[derive(Default, Debug, Clone)]
struct FlexibleTimeReservationSystemMetadata {
    mapping: HashMap<usize, usize>,
}

pub struct FlexibleTimeReservationSystem<ClockType = DefaultUtcClock> {
    record: HashMap<usize, Vec<ReservationRequest>>,
    claims: HashMap<usize, super::algorithms::sat_flexible_time_model::Assignment>,
    max_id: usize,
    async_executor: AsyncExecutor<
        super::algorithms::sat_flexible_time_model::Problem,
        FlexibleTimeReservationSystemMetadata,
    >,
    wait_point_system: WaitPointSystem,
    clock_source: ClockType,
}

impl<
        ClockType: ClockSource + Default + Clone + std::marker::Send + std::marker::Sync + 'static,
    > Default for FlexibleTimeReservationSystem<ClockType>
{
    fn default() -> Self {
        let mut alg_pool =
            AlgorithmPool::<super::algorithms::sat_flexible_time_model::Problem>::default();
        alg_pool.add_algorithm(Arc::new(SATFlexibleTimeModel {
            clock_source: ClockType::default(),
        }));

        Self {
            record: Default::default(),
            claims: Default::default(),
            max_id: 0,
            async_executor: AsyncExecutor::init(alg_pool),
            wait_point_system: Default::default(),
            clock_source: ClockType::default(),
        }
    }
}

impl<ClockType: ClockSource + Clone + std::marker::Send + std::marker::Sync + 'static>
    FlexibleTimeReservationSystem<ClockType>
{
    pub fn create_with_clock(clock_source: ClockType) -> Self {
        let mut alg_pool =
            AlgorithmPool::<super::algorithms::sat_flexible_time_model::Problem>::default();
        alg_pool.add_algorithm(Arc::new(SATFlexibleTimeModel {
            clock_source: clock_source.clone(),
        }));

        Self {
            record: Default::default(),
            claims: Default::default(),
            max_id: 0,
            async_executor: AsyncExecutor::init(alg_pool),
            wait_point_system: Default::default(),
            clock_source,
        }
    }

    pub fn request_resources(
        &mut self,
        alternatives: Vec<ReservationRequest>,
    ) -> Result<Ticket, &'static str> {
        self.record.insert(self.max_id, alternatives);
        let result = Ticket { count: self.max_id };
        self.max_id += 1;

        let snapshot = self.get_snapshot();
        self.async_executor.attempt_solve(snapshot);

        Ok(result)
    }

    pub fn claim_request(
        &mut self,
        ticket: Ticket,
        safe_spot: &Vec<String>,
    ) -> Result<ClaimSpot, &str> {
        if self.claims.contains_key(&ticket.get_id()) {
            return Err("Ticket already claimed");
        }

        if safe_spot.len() == 0 {
            println!("You should include wait spots otherwise, it may lead to deadlock");
        }

        if let Some(rec) = self.record.get(&ticket.get_id()) {
            if rec.len() == 0 {
                println!("No alternatives requested, moving to waitpoint only");
                let wait_points: Vec<_> = safe_spot
                    .iter()
                    .map(|resource| WaitPointRequest {
                        wait_point: resource.clone(),
                        time: self.clock_source.now(),
                    })
                    .collect();

                let Ok(ticket) = self.wait_point_system.request_waitpoint(&wait_points) else {
                    println!("Cloud not allocate to any of: {:?}", safe_spot);
                    return Err("Could not allocate any wait points. Are we sure there are enough waitpoints available?");
                };
                return Ok(ClaimSpot::WaitPermanently(ticket.selected_index));
            }
        } else {
            return Err("Never issued this ticket before");
        }

        let Some((result, metadata)) = self.async_executor.retrieve_feasible_schedule() else {
            println!("Warning: solver has not concluded any solution yet. Please listen for solutions when ready. For now proceed to wait point.");
            let wait_points: Vec<_> = safe_spot
                .iter()
                .map(|resource| WaitPointRequest {
                    wait_point: resource.clone(),
                    time: self.clock_source.now(),
                })
                .collect();

            let Ok(ticket) = self.wait_point_system.request_waitpoint(&wait_points) else {
                return Err("Could not allocate any wait points. Are we sure there are enough waitpoints available?");
            };
            return Ok(ClaimSpot::WaitPermanently(ticket.selected_index));
        };

        println!("Schedule: {:?}", result);

        let Some(request_idx) = metadata.mapping.get(&ticket.get_id()) else {
            // This hsould never happen therefore panic instead of error.
            panic!("Metadata was malformed");
        };

        // TODO(arjoc): Inefficient: We should have better data structure to back this
        for (resource, schedule) in result {
            for item in schedule {
                if item.id.0 == *request_idx {
                    println!("Got result {:?}", item);
                    // TODO(arjoc): Don't hard code
                    if (item.start_time - self.clock_source.now())
                        .num_seconds()
                        .abs()
                        > 20
                    {
                        let wait_points: Vec<_> = safe_spot
                            .iter()
                            .map(|resource| WaitPointRequest {
                                wait_point: resource.clone(),
                                time: self.clock_source.now(),
                            })
                            .collect();

                        let Ok(ticket) = self.wait_point_system.request_waitpoint(&wait_points)
                        else {
                            return Err("Could not allocate any wait points. Are we sure there are enough waitpoints available?");
                        };
                        return Ok(ClaimSpot::WaitAtThenGo(
                            ticket.selected_index,
                            Goal {
                                resource,
                                satisfies_alt: item.id.1,
                                time: item.start_time,
                            },
                        ));
                    } else {
                        return Ok(ClaimSpot::GoImmediately(Goal {
                            resource,
                            satisfies_alt: item.id.1,
                            time: item.start_time,
                        }));
                    }
                }
            }
        }

        return Err("We should never reach here. Something went wrong internally");
    }

    pub fn request_waitpoint(&mut self, wait_points: &Vec<String>) -> Result<usize, String> {
        let wait_points: Vec<_> = wait_points
            .iter()
            .map(|resource| WaitPointRequest {
                wait_point: resource.clone(),
                time: self.clock_source.now(),
            })
            .collect();
        let result = self.wait_point_system.request_waitpoint(&wait_points);
        match result {
            Ok(res) => Ok(res.selected_index),
            Err(desc) => Err(desc),
        }
    }

    pub fn release_waitpoint(&mut self, wait_point: &String) {
        self.wait_point_system
            .release_waitpoint_at_time(wait_point, &self.clock_source.now());
    }

    fn get_snapshot(
        &mut self,
    ) -> Snapshot<
        super::algorithms::sat_flexible_time_model::Problem,
        FlexibleTimeReservationSystemMetadata,
    > {
        let mut requests = vec![];
        let mut mapping = HashMap::<usize, usize>::new();

        println!("Sending: {:?}", self.record);

        for (key, alts) in &self.record {
            mapping.insert(requests.len(), *key);
            if let Some(reservation_state) = self.claims.get(&key) {
                let mut selected = alts[reservation_state.id.1].clone();
                selected.parameters.start_time =
                    StartTimeRange::exactly_at(&reservation_state.start_time);
                requests.push(vec![selected]);
            } else {
                requests.push(alts.clone());
            }
        }

        let problem = super::algorithms::sat_flexible_time_model::Problem { requests };

        Snapshot {
            problem,
            metadata: FlexibleTimeReservationSystemMetadata { mapping },
        }
    }
}

/*
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
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let claim = res_sys.claim_request(ticket).unwrap();
}*/

#[cfg(test)]
#[test]
fn test_sat_flexible_time_model() {
    use crate::cost_function::static_cost::StaticCost;

    let now = Utc::now();
    let mut flexible_ressys: FlexibleTimeReservationSystem<DefaultUtcClock> =
        FlexibleTimeReservationSystem::default();

    let alternatives1 = vec![ReservationRequest {
        parameters: crate::ReservationParameters {
            resource_name: "res1".to_string(),
            duration: Some(Duration::minutes(10)),
            start_time: StartTimeRange {
                earliest_start: Some(now + Duration::seconds(180)),
                latest_start: Some(now + Duration::seconds(200)),
            },
        },
        cost_function: Arc::new(StaticCost::new(2.0)),
    }];
    let ticket1 = flexible_ressys.request_resources(alternatives1).unwrap();

    let alternatives2 = vec![ReservationRequest {
        parameters: crate::ReservationParameters {
            resource_name: "res2".to_string(),
            duration: Some(Duration::minutes(10)),
            start_time: StartTimeRange {
                earliest_start: Some(now + Duration::seconds(180)),
                latest_start: Some(now + Duration::seconds(200)),
            },
        },
        cost_function: Arc::new(StaticCost::new(2.0)),
    }];
    let ticket2 = flexible_ressys.request_resources(alternatives2).unwrap();

    let safe_spots = vec!["1".to_string(), "2".to_string()];

    // 400 milliseconds is enough for the solver hopefully
    std::thread::sleep(std::time::Duration::from_millis(1000));

    let res1 = flexible_ressys.claim_request(ticket2, &safe_spots).unwrap();
    let res2 = flexible_ressys.claim_request(ticket1, &safe_spots).unwrap();

    assert!(matches!(res1, ClaimSpot::WaitAtThenGo(_x, _y)));
    assert!(matches!(res2, ClaimSpot::WaitAtThenGo(_x, _y)));
}

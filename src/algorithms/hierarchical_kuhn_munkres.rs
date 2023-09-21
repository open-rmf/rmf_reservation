use core::panic;
use std::{collections::{HashMap, BTreeMap, HashSet, BinaryHeap}, hash::{Hash, self}, cmp::Ordering, sync::Arc, ops::Bound};

use chrono::{DateTime, Utc, TimeZone, Duration};
use fnv::{FnvBuildHasher, FnvHashSet};
use ordered_float::OrderedFloat;
use rand::Rng;

use crate::{ReservationRequest, utils::multimap::UniqueMultiHashMap, cost_function::static_cost::StaticCost, ReservationSchedule};

struct FakeResourceMetaInfo {
    original_resource_name: String,
    original_resource_id: usize,
    original_request: (usize, usize),
}

pub struct TimeBasedBranchAndBound {
    base_resources: Vec<String>,
    last_request_id: usize,
    resource_name_to_id: HashMap<String, usize>,
    requests: HashMap<usize, Vec<ReservationRequest>>,
    // Maps requests by (request_id, resource_id) -> index in requests table
    request_reservation_idx: HashMap<(usize, usize), usize>,
}

impl TimeBasedBranchAndBound {
    pub fn create_with_resources(resources: &Vec<String>) -> Self {
        Self {
            base_resources: resources.clone(),
            last_request_id: 0,
            resource_name_to_id: HashMap::from_iter(
                resources
                    .iter()
                    .enumerate()
                    .map(|(size, str)| (str.clone(), size)),
            ),
            requests: HashMap::new(),
            request_reservation_idx: HashMap::new(),
        }
    }

    // Requires that ReservationRequests have a well defined start time
    pub fn request_resources(&mut self, request: Vec<ReservationRequest>) -> Option<usize> {
        let req_id = self.last_request_id;
        for r_id in 0..request.len() {
            let resource = request[r_id].parameters.resource_name.clone();
            let Some(&resource_id) = self.resource_name_to_id.get(&resource) else {
                return None; 
            };
            self.request_reservation_idx
                .insert((req_id, resource_id), r_id);
        }
        self.last_request_id += 1;
        self.requests.insert(req_id, request);
        Some(req_id)
    }

    pub fn generate_literals_and_remap_requests(&self) -> SparseScheduleConflictHillClimber {
        let mut fake_resources = vec![];
        let mut fake_resource_mapping: HashMap<String, FakeResourceMetaInfo> = HashMap::new();
        let mut fake_requests: HashMap<usize, Vec<ReservationRequest>> = HashMap::new();


        let mut id_to_res = HashMap::new();
        let mut res_to_id = HashMap::new();

        // HashMap<Resources, BTReeMAp<StartTime, Vec<(Req_id, alt_id, fake_resource_name, end_time)>>>
        let mut start_time_per_resource: HashMap<String, BTreeMap<DateTime<Utc>, Vec<(usize, usize, String, DateTime<Utc>)>>> = HashMap::new();

        for (req_id, requests) in &self.requests {
            let mut curr_req_alt = vec![];
            for i in 0..requests.len() {
                let request =  &requests[i];
                let resource_id = self.resource_name_to_id[&request.parameters.resource_name];
                
                // Generate new resource names
                let res_name = format!("{} {} {}", resource_id, req_id, i);
                id_to_res.insert((*req_id, i), res_name.clone());
                res_to_id.insert(res_name.clone(), (*req_id, i));
                fake_resources.push(res_name.clone());
                fake_resource_mapping.insert(res_name.clone(), FakeResourceMetaInfo {
                    original_resource_name: request.parameters.resource_name.clone(),
                    original_resource_id: resource_id,
                    original_request: (*req_id, i),
                });

                let mut fake_req = request.clone();
                fake_req.parameters.resource_name = res_name.clone();
                curr_req_alt.push(fake_req);


                // Build trees for calculating conflict
                // Note: we assume there is a well defined earliest start and that
                // start is the start time.
                let start = request.parameters.start_time.earliest_start.unwrap();

                // We also assume that all reservation requests come with a duration
                let end = start + request.parameters.duration.unwrap();

                if let Some(mut schedule ) = start_time_per_resource.get_mut(&request.parameters.resource_name) {
                    if let Some(mut bucket) = schedule.get_mut(&start) {
                        bucket.push((*req_id, i, res_name.clone(), end.clone()));
                    }
                    else{
                        schedule.insert(start.clone(), vec![(*req_id, i, res_name.clone(), end.clone())]);
                    }
                }
                else {
                    let mut btree = BTreeMap::new();
                    btree.insert(start.clone(), vec![(*req_id, i, res_name.clone(), end.clone())]);
                    start_time_per_resource.insert(request.parameters.resource_name.clone()
                        , btree);
                };
            }
            fake_requests.insert(*req_id, curr_req_alt);
        }

        // TODO(arjo) - This is very inefficient...
        let mut conflict_sets: HashMap<String, HashSet<String>> = HashMap::new();
        for resource in &self.base_resources {
            let Some(start_sched) = start_time_per_resource.get(resource) else {
                continue;
            };

            for (start_time, reservation) in start_sched {
                for (req_id, alt_id, res_name, end) in reservation {
                    
                    let mut conflicts = HashSet::new();

                    // We also assume that all reservation requests come with a duration
                    for (other_start_time, reservations) in start_sched {
                        if other_start_time > end {
                            break;
                        }
                        
                        for (other_req_id, other_alt_id, other_res_name, other_end) in reservations {
                            if other_res_name == res_name {
                                continue;
                            }
                            
                            // Case 1: Reservation starts during the current request in qn
                            if other_start_time >= start_time && other_start_time <= end {
                                conflicts.insert(other_res_name.clone());
                            }

                            // Case 2: Other start time is before my start time and ends after it
                            if other_start_time < start_time && other_end > start_time {
                                conflicts.insert(other_res_name.clone());
                            }
                        }
                    }
                    
                    for other_resource in &conflicts {
                        if let Some(mut conflict_set) = conflict_sets.get_mut(other_resource) {
                            conflict_set.insert(res_name.clone());
                        }
                        else {
                            conflict_sets.insert(other_resource.clone(), HashSet::from_iter([res_name.clone()]));
                        }
                    }

                    if let Some(mut conflict_set) = conflict_sets.get_mut(res_name) {
                        conflict_set.extend(conflicts.iter().map(|p| p.clone()));
                    }
                    else {
                        conflict_sets.insert(res_name.clone(), conflicts);
                    }
                }
            }
        }

        SparseScheduleConflictHillClimber {
            id_to_res,
            res_to_id,
            conflict_sets,
            fake_requests,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ConstraintList {
    positive_constraints: HashMap<usize, usize>, 
    negative_constraints: HashSet<String>
}

impl Hash for ConstraintList {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        for p in &self.positive_constraints {
            p.hash(state);
        }

        for n in &self.positive_constraints {
            n.hash(state);
        }
    }
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct Solution {
    //assignments: HashMap<usize, usize>,
    cost: OrderedFloat<f64>,
    unallocated: usize,
    positive_constraints: HashMap<usize, usize, FnvBuildHasher>, 
    negative_constraints: HashSet<String, FnvBuildHasher>,
    conflicts: HashSet<(usize, usize)>,
    starvation_groups_found: Vec<StarvationGroup>
}

impl Hash for Solution {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.unallocated);
        for (k,v) in &self.positive_constraints {
            state.write_usize(*k);
            state.write_usize(*v);
        }

        for str in &self.negative_constraints {
            state.write_str(&str);
        }
    }
}

impl Ord for Solution {

    // Flip comparison to make binary heap a min heap
    fn cmp(&self, other: &Self) -> Ordering {
        other.unallocated.cmp(&self.unallocated)
            .then_with(|| other.cost.cmp(&self.cost))
            .then_with(|| other.conflicts.len().cmp(&self.conflicts.len()))
    }
}

impl PartialOrd for Solution {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StarvationGroup {
    positive: HashSet<(usize, usize), FnvBuildHasher>,
}
impl Hash for StarvationGroup {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        for (k,v) in &self.positive {
            state.write_usize(*k);
            state.write_usize(*v);
        }
    }
}

#[derive(Debug)]
pub struct SparseScheduleConflictHillClimber {
    res_to_id: HashMap<String, (usize, usize)>,
    id_to_res: HashMap<(usize, usize), String>,
    conflict_sets: HashMap<String, HashSet<String>>,
    fake_requests: HashMap<usize, Vec<ReservationRequest>>,
}

impl SparseScheduleConflictHillClimber {
    pub fn debug_print(&self) {
        for (req_id, alts) in &self.fake_requests{
            for alt in alts {
                println!("{:?} {:?} {:?}  For {:?}", req_id, alt.parameters.resource_name, alt.parameters.start_time.earliest_start, alt.parameters.start_time.earliest_start.unwrap() + alt.parameters.duration.unwrap());
            }
        }
    }

    pub fn literals(&self) -> HashMap<usize, usize> {
        self.fake_requests.iter().map(|(k,v)| (*k, v.len())).collect()
    } 

    fn backtrack(&self, assignments: &mut Vec<Option<usize>>, solution: &Solution, implications: &UniqueMultiHashMap<(usize, usize), String>, backtracker: &UniqueMultiHashMap<String, (usize, usize)>, starvation_sets: &mut HashSet<StarvationGroup>) -> Solution {
        
        let mut positive_constraints  = solution.positive_constraints.clone();
        let mut backtrack = vec![];
        for group in &solution.starvation_groups_found {
            for (k, v) in &group.positive {
                let Some(v2) = positive_constraints.get(k) else {
                    continue;
                };
                if *v == *v2 {
                    positive_constraints.remove(&k);
                    backtrack.push((*k,*v));
                }
            }
        }
        let mut negative_constraints: HashSet<_, FnvBuildHasher> = positive_constraints.iter().map(|(k,v)| 
         {
            let mut constraints = vec![];
            if let Some(imp) = implications.get(&(*k, *v)) {
                constraints.extend(imp.iter().map(|v| v.clone()))
            }
            constraints}).flatten().collect();
        for bt in backtrack {
            let id = &self.id_to_res[&bt];
            negative_constraints.insert(id.clone());
        }
        self.greedy_allocate(assignments, positive_constraints, negative_constraints, implications, backtracker, starvation_sets)
    }

    pub(crate)fn score_cache(&self) -> HashMap<(usize, usize), f64> {
        let mut hashmap = HashMap::new();
        for (req_id, res) in &self.fake_requests {
            for res_id in  0..res.len() {
                let instant = res[res_id].parameters.start_time.earliest_start.unwrap();
                hashmap.insert((*req_id, res_id), res[res_id].cost_function.cost(&res[res_id].parameters, &instant));
            }
        }
        hashmap
    }

    fn greedy_allocate(&self,
        assignments: &mut Vec<Option<usize>>, 
        positive_constraints: HashMap<usize, usize, FnvBuildHasher>, 
        negative_constraints: HashSet<String, FnvBuildHasher>,
        implications: &UniqueMultiHashMap<(usize, usize), String>,
        backtracker: &UniqueMultiHashMap<String, (usize, usize)>,
        starvation_sets: &mut HashSet<StarvationGroup>) -> Solution {
        
        let mut total_score = 0.0;
        let mut unallocated = 0usize;
        let mut starvation_groups_found = vec![];
        for (req_id, res) in &self.fake_requests {
            if let Some(alt) = positive_constraints.get(&req_id) {
                assignments[*req_id] = Some(*alt);
                let req = &res[*alt];
                total_score += req.cost_function.cost(&req.parameters, 
                    &req.parameters.start_time.earliest_start.unwrap());
                continue;
            }

            let mut min_score = f64::INFINITY;
            let mut selected_alt = None;
            for alt in  0..res.len() {
                let req = &res[alt];
                let score = req.cost_function.cost(&req.parameters, 
                    &req.parameters.start_time.earliest_start.unwrap());
                let resource = res[alt].parameters.resource_name.clone();
                if negative_constraints.contains(&resource) {
                    assignments[*req_id] = None;
                    continue;
                }

                if min_score > score {
                    min_score = score;
                    selected_alt = Some(alt);
                }
            }

            let Some(alt) = selected_alt else {
                // back track and find banned groups
                let mut starvation_group: HashSet<(usize, usize), FnvBuildHasher> = fnv::FnvHashSet::default();
                for alt in  0..res.len() {
                    let resource = &self.id_to_res[&(*req_id, alt)];
                    let Some(idx) = backtracker.get(resource) else {
                        continue;
                    };
                    starvation_group.extend(idx.iter());
                }

                println!("Got starvation group {:?}", starvation_group);
                starvation_sets.insert(StarvationGroup{ positive: starvation_group.clone()});
                starvation_groups_found.push(StarvationGroup{ positive: starvation_group.clone()});

                unallocated += 1;
                continue;
            };
            total_score += min_score;

            assignments[*req_id] = Some(alt);
        }

        Solution {
            //assignments, 
            cost: OrderedFloat(total_score), 
            unallocated,
            positive_constraints,
            negative_constraints,
            conflicts: self.get_conflicts(&assignments, implications),
            starvation_groups_found
        }
    }

    pub fn get_banned_reservation_combinations(&self) -> UniqueMultiHashMap<(usize, usize), (usize, usize)> {
       let (implications, _) = self.get_implications();
       let mut final_implications = UniqueMultiHashMap::new();
       
       for (index, imp) in implications.iter() {
            for i in imp {
                final_implications.insert(*index, self.res_to_id[i]);
            }
       }
       final_implications
    }

    /// Get implied banned stuff
    fn get_implications(&self) -> (UniqueMultiHashMap<(usize, usize), String>, UniqueMultiHashMap<String, (usize,usize)>) {
        // Contains a  bunch of banned allocations
        // <Contenting allocation, resources_to_ban>
        let mut banned_allocations = UniqueMultiHashMap::new();
        let mut backtrace = UniqueMultiHashMap::new();
        
        for (req_id, alt) in &self.fake_requests {
            for alt_id in 0..alt.len() {
                let fake_req = &self.fake_requests[req_id][alt_id];
                if let Some(conflicts) = self.conflict_sets.get(&fake_req.parameters.resource_name) {
                    for conflict in conflicts {
                        banned_allocations.insert((*req_id, alt_id), conflict.clone());
                        backtrace.insert(conflict.clone(), (*req_id, alt_id));
                    }
                }
                banned_allocations.insert((*req_id, alt_id), fake_req.parameters.resource_name.clone());
                backtrace.insert(fake_req.parameters.resource_name.clone(), (*req_id, alt_id));
            }
        }
        (banned_allocations, backtrace)
    }



    fn get_conflicts(&self, allocations: &Vec<Option<usize>>, implications: &UniqueMultiHashMap<(usize, usize), String>) -> HashSet<(usize, usize)>{
        let mut conflicts = HashSet::new();

        // O(n^2) run time. TODO(arjo) : reduce
        /*for req_id1 in 0..allocations.len() {
            let Some(alt_id1) = allocations[req_id1] else {
                continue;
            };
            let Some(imp1) = implications.get(&(req_id1, alt_id1)) else {
                continue;
            };
            
            for req_id2 in 0..allocations.len() {
                let Some(alt_id2) = allocations[req_id2] else {
                    continue;
                };
                if req_id1 == req_id2 {
                    // No need to check alternatives as only one request will be satisfied
                    continue;
                }

                if imp1.contains(&self.id_to_res[&(req_id2, alt_id2)]) {
                    conflicts.insert((req_id1, alt_id1));
                    conflicts.insert((req_id2, alt_id2));
                }
            }
        }*/

        let mut seen = HashSet::new();
        for req_id1 in 0..allocations.len() {
            let Some(alt_id1) = allocations[req_id1] else {
                continue;
            };
            let res_name = &self.id_to_res[&(req_id1, alt_id1)];
            let Some(imp1) = self.conflict_sets.get(res_name) else {
                continue;
            };

            for q in imp1.intersection(&seen) {
                conflicts.insert((req_id1, alt_id1));
                conflicts.insert(self.res_to_id[q]);
            } 
            
            seen.insert(res_name.clone());
        }

        conflicts
    }

    /// Check conflict by remapping timelines
    pub fn solve(&self, hint: HashMap<usize, usize, FnvBuildHasher>,) -> Option<(Solution, Vec<Option<usize>>)> {
        let mut assignments =vec![None; self.fake_requests.len()];

        let mut explored = HashSet::new();
        let mut starvation_sets = HashSet::new();
        
        let (implications, backtracker) = self.get_implications();
        
        let mut solution =
            self.greedy_allocate(&mut assignments, hint, fnv::FnvHashSet::default(), &implications, &backtracker, &mut starvation_sets);
        solution.positive_constraints.clear();
        
        for (given, disallowed) in implications.iter() {
            println!("Given {:?} => Not allowed {:?}", given, disallowed);
        }

        println!("Starting with: {:?}", solution); 
        let mut priority_queue = BinaryHeap::new();
        priority_queue.push(solution);

        while let Some(solution) = priority_queue.pop() {
            
            if solution.conflicts.len() == 0 && solution.unallocated == 0 {
                let solution = self.greedy_allocate(&mut assignments, solution.positive_constraints, solution.negative_constraints, &implications, &backtracker, &mut starvation_sets);
                return Some((solution.clone(), assignments));
            }

            let options = solution.conflicts.iter().map(|&(req_id, alt)| {
                let mut pt = HashSet::new();
                if let Some(x)  = implications.get(&(req_id, alt)) {
                  pt.extend(x.iter())
                }
                pt 
            }).flatten().map(|f| self.res_to_id[f]);

            for (req_id, alt) in options {
                let mut positive_constraints = solution.positive_constraints.clone();
                let mut negative_constraints = solution.negative_constraints.clone();
                
                
                positive_constraints.insert(req_id, alt);

                // Check starvation sets
               /* let starved: bool = {
                    let mut val = false;
                    for starvation_group in &starvation_sets {
                        let mut hashset = FnvHashSet::from_iter(positive_constraints.iter().map(|(k,v)| (*k, *v)));
                        if hashset.intersection(&starvation_group.positive).count() == starvation_group.positive.len() {
                            val =true;
                        }
                    }
                    val
                };
                if starved {
                    println!("Dropping solution cause it will starve resources");
                    continue;
                }*/

                if let Some(banned_resources) = implications.get(&(req_id, alt)) {
                    negative_constraints.extend(banned_resources.iter().filter(|&p| self.res_to_id[p] != (req_id, alt)).map(|p| p.clone()));

                }
                
                let solution = self.greedy_allocate(&mut assignments,positive_constraints, negative_constraints, &implications, &backtracker, &mut starvation_sets);
                if !explored.contains(&solution) && solution.unallocated == 0 {
                    println!("Adding solution {:?}", solution);
                    priority_queue.push(solution.clone());
                }
                else {
                    if solution.unallocated > 0 {
                        println!("Got unallocated {:?}", solution);

                        let new_soln = self.backtrack(&mut  assignments, &solution, &implications, &backtracker, &mut starvation_sets);

                        println!("Back-tracking to {:?}", new_soln);
                        if new_soln.conflicts.len() == 0 && new_soln.unallocated == 0 {
                            return Some((new_soln.clone(), assignments));
                        }

                        if !explored.contains(&new_soln) {
                            //priority_queue.push(new_soln.clone());
                            explored.insert(new_soln);
                        }
                    }
                    println!("Seen");
                }

                if solution.conflicts.len() == 0 && solution.unallocated == 0 {
                    return Some((solution.clone(), assignments));
                }
                explored.insert(solution);
            }
        }
        None
    }
}


#[cfg(test)]
#[test]
fn test_conflict_checker() {
    use std::sync::Arc;
    use chrono::{TimeZone, Duration};
    use fnv::FnvHashMap;

    use crate::{ReservationParameters, StartTimeRange, cost_function::static_cost::StaticCost};


    let resources = vec!["station1".to_string(), "station2".to_string()];
    let alternative1 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: resources[0].clone(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(StaticCost::new(3.0)),
    };

    let alternative2 = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: resources[1].clone(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(StaticCost::new(6.0)),
    };


    let alternative1_cheaper = ReservationRequest {
        parameters: ReservationParameters {
            resource_name: resources[0].clone(),
            start_time: StartTimeRange {
                earliest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap()),
                latest_start: Some(Utc.with_ymd_and_hms(2023, 7, 8, 7, 10, 11).unwrap()),
            },
            duration: Some(Duration::minutes(30)),
        },
        cost_function: Arc::new(StaticCost::new(2.0)),
    };


    let req1 = vec![alternative1, alternative2];
    let req2 = vec![alternative1_cheaper];

    let mut system = TimeBasedBranchAndBound::create_with_resources(&resources);

    system.request_resources(req1);
    system.request_resources(req2);

    let solver = system.generate_literals_and_remap_requests();
    //println!("Generated literals");
    solver.debug_print();
    let (solution, _) = solver.solve(FnvHashMap::default()).unwrap();
    println!("{:?}", solution);
    assert!((solution.cost.0 - 8.0).abs() < 1.0);
}


pub fn generate_test_scenario_with_known_best(num_resources: usize, max_requests_per_resource: usize, num_conflicts: usize) -> (Vec<Vec<ReservationRequest>>, Vec<String>) {

    let resources = Vec::from_iter(
        (0..num_resources).map(|m| format!("Station {}", m)));
    let mut rng = rand::thread_rng();
    
    let mut results_vec = vec![];

    let mut reservations_schedule = HashMap::new();
    // Build correct solution
    for res in &resources {
        let res_size = rng.gen_range(0..=max_requests_per_resource);

        let start_time = Utc.with_ymd_and_hms(2023, 7, 8, 6, 10, 11).unwrap();
        let mut last_start_time = start_time;
        reservations_schedule.insert(res.clone(), ReservationSchedule::new());
        for i in 0..res_size {
            last_start_time += Duration::minutes(rng.gen_range(0..60));
            let duration = Duration::minutes(rng.gen_range(20..90));
            let request = ReservationRequest {
                parameters: crate::ReservationParameters { 
                    resource_name: res.clone(), duration: Some(duration), start_time: crate::StartTimeRange { earliest_start: Some(last_start_time), latest_start:  Some(last_start_time)} },
                    cost_function: Arc::new(StaticCost::new(1.0))
            };

            let Some(schedule )= reservations_schedule.get_mut(res) else {
                panic!("Should never get here");
            };
            schedule.insert((i,0), Some(duration), last_start_time);
            last_start_time += duration;

            results_vec.push(vec![request]);
        }
    }

    // Add conflicts for fun at lower cost
    for _ in 0..num_conflicts {
        let res_id = rng.gen_range(0..resources.len());
        let resource_name = &resources[res_id];
        let Some(schedule) = reservations_schedule.get(resource_name) else {
            continue;
        };

        if schedule.schedule.len() == 0 {
            continue;
        }

        let res_id = if schedule.schedule.len() > 1 {rng.gen_range(1..schedule.schedule.len())} else {1};
        let mut idx = 0;
        let mut prev_time = *schedule.schedule.first_key_value().unwrap().0;
        for s  in &schedule.schedule {
            if idx == res_id {
                // Lets create a reservation to disrupt the previous two reservations
                let req = ReservationRequest {
                    parameters: crate::ReservationParameters {
                        resource_name: resource_name.clone(), 
                        duration: Some((*s.0 + s.1.2.unwrap()) - prev_time), 
                        start_time: crate::StartTimeRange::exactly_at(&prev_time)
                    },
                    cost_function: Arc::new(StaticCost::new(0.9))
                };
                let rn = rng.gen_range(0..results_vec.len());
                results_vec[rn].push(req);
            }
            idx += 1;
            prev_time = *s.0;
        }
    }

    (results_vec, resources)
}

fn validate_optimal_solution(soln: &(Vec<Vec<ReservationRequest>>, Vec<String>)) {

}

#[cfg(test)]
use test::Bencher;
#[cfg(test)]
#[test]
fn test_generation() {
    use fnv::FnvHashMap;

    let (requests, resources) =  generate_test_scenario_with_known_best(4, 4, 3);
    //println!("Requests {:?}", requests);
    
    let mut system = TimeBasedBranchAndBound::create_with_resources(&resources);
    for req in requests {
        system.request_resources(req);
    }
   // b.bench(|_| {
        let soln = system.generate_literals_and_remap_requests();
        soln.debug_print();
        let _ = soln.solve(FnvHashMap::default()).unwrap();
       // Ok(())
    //});
}
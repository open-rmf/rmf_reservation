// The goal of this example is to demonstrate fire safety
// Each robot has a seperate hypothetical spot in a single story
// warehouse. In the backend an optimizer runs giving 
// the best possible assignment to each robot.
// We compare performance between
// with Conflict-based-search in the loop and
// without conflict based search in the loop.
// This uses the trivial heuristic of dijkstra to 
// the nearest point in the graph.
#![feature(map_try_insert)]
use std::{collections::{HashMap, HashSet, BTreeMap}, hash::Hash, sync::Arc, rc::Rc};

use mapf::{prelude::SimpleGraph, motion::{r2::{Position, LineFollow, WaypointR2}, SpeedLimit, Trajectory}, templates::InformedSearch, Planner, prelude::{SharedGraph, AStar}};
use ordered_float::OrderedFloat;
use pathfinding::prelude::kuhn_munkres_min;
use rmf_site_format::legacy::nav_graph::NavGraph;

use rand::prelude::*;
use rand_seeder::{Seeder, SipHasher};
use rand_pcg::Pcg64;

struct RobotPosition {
    from: usize,
    to: usize,
    distance: f64
}

fn seed_robots(num_robots: usize, graph: &SimpleGraph<Position, SpeedLimit>, generator: &mut Pcg64) -> Vec<RobotPosition> { 
    let mut result = vec![];
    for _ in 0..num_robots {
        let len = graph.vertices.len();
        let from = generator.gen_range(0..len);
        let Some(&(to, _)) = graph.edges[from].choose(generator) else {
            continue;
        };
        let distance = generator.gen_range(0.0..=1.0);

        let robot = RobotPosition {
            from, to, distance
        };
        result.push(robot);
    }
    result
}

#[derive(Clone, Eq, PartialEq)]
struct AssignmentState {
    assignment : HashMap<usize, Vec<usize>>, // Index is parking spot. Value is robot size.
}

impl AssignmentState {
    pub fn is_valid(&self) -> bool {
        for (_, v) in self.assignment {
            if v.len() > 1 {
                return false;
            }
        }
        true
    }
    pub fn split_by_conflicts(&self,
        enforced: &HashSet<(usize, usize)>,
        parking_spots: &HashSet<usize>) -> Vec<(AssignmentState, HashSet<(usize, usize)>)> 
    {

    }
}
impl std::hash::Hash for AssignmentState {
    fn hash<H>(&self, state: &mut H)
    where
        H: std::hash::Hasher {
        state.write_usize(self.assignment.len());
        for (robot, parkings) in self.assignment {
            state.write_usize(robot);
            for parking in parkings {
                state.write_usize(parking);
            }
        }
    }
}

#[derive(Clone)]
struct ConstraintContainer {
    cache_cell: Rc<CacheCell>,
    assignment: AssignmentState,
    constraints_to_keep_same: HashSet<(usize, usize)>,
    constraints_to_change: HashSet<(usize, usize)>,
    initial_positions: Rc<Vec<RobotPosition>>,
    cost: OrderedFloat<f64>,
}

impl Hash for ConstraintContainer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.assignment.hash(state);
    }
}

impl PartialEq for ConstraintContainer {
    fn eq(&self, other: &Self) -> bool {
        self.assignment == other.assignment
    }
}

impl Eq for ConstraintContainer {}

impl PartialOrd for ConstraintContainer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.cost.partial_cmp(&other.cost)
    }
}

impl ConstraintContainer {

    pub fn new(cache_cell: Rc<CacheCell>, assignment: AssignmentState,  initial_positions: Rc<Vec<RobotPosition>>) -> Self {
        let mut res = Self {
            cache_cell,
            assignment,
            initial_positions,
            constraints_to_change: HashSet::new(),
            constraints_to_keep_same: HashSet::new(),
            cost: OrderedFloat(0.0)
        };

        res.cost = res.cost();

        res
    }
    fn cost(&self) -> OrderedFloat<f64> {
        let mut total_cost = 0.0f64;
        for robot in 0..self.assignment.assignment.len() {
            for parking in 0..self.assignment.assignment[robot] {
                total_cost += self.cache_cell.cost_given_position(&self.initial_positions[robot], &parking_spot).0;
            }
        }
        OrderedFloat(total_cost)
    }
}

#[derive(Clone)]
struct CacheCell {
    path_to_index: HashMap<(usize, usize), usize>,
    cost_to_edge: HashMap<usize, Vec<(usize, OrderedFloat<f64>)>>,
    graph: SimpleGraph<Position, SpeedLimit>
}

impl CacheCell {
    fn from(cache_builder: &HashMap<usize, BTreeMap<OrderedFloat<f64>, usize>>, graph: SimpleGraph<Position, SpeedLimit>) -> Self {
        
        let mut res = Self {
            path_to_index: HashMap::new(),
            cost_to_edge: HashMap::new(),
            graph
        };
        
        for (from, costs) in cache_builder {
            let Ok(m_vec) = res.cost_to_edge.try_insert(*from, vec![]) else {
                panic!("Spooky action at a distance");
            };

            for (cost, to) in costs {
                res.path_to_index.insert((*from, *to), m_vec.len());
                m_vec.push((*to, *cost));
            }
        }

        res
    }
    fn cost_given_index(&self, from: &usize, to: &usize) -> OrderedFloat<f64> {
        let Some(&index) = self.path_to_index.get(&(*from, *to)) else {
            return OrderedFloat(f64::INFINITY);
        };

        let Some(cost) = self.cost_to_edge.get(from) else {
            return OrderedFloat(f64::INFINITY);
        };
        
        let (_, cost) = cost[index];
        cost
    }

    fn cost_given_position(&self, position: &RobotPosition, goal: &usize) -> OrderedFloat<f64>
    {
        let cost_at_start = self.cost_given_index(&position.from, goal);
        let cost_at_end = self.cost_given_index(&position.to, goal);
        
        let vert1 = self.graph.vertices[position.from];
        let vert2 = self.graph.vertices[position.to];
        let length = (vert1 - vert2).norm();

        let l1 = cost_at_start + OrderedFloat(length * position.distance);
        let l2 = cost_at_end + OrderedFloat(length * (1.0 - position.distance));
        l1.min(l2)
    }

}

fn main() -> std::io::Result<()> {
    let s = std::fs::read_to_string("~/Desktop/office.real.nav.yaml")?;
    let nav: Result<NavGraph, serde_yaml::Error> = serde_yaml::from_str(&s);

    let Ok(nav) = nav else {
        return Ok(())
    };

    let mut parking_spots = HashSet::new();
    let mut level_map: HashMap<&String, SimpleGraph<Position, SpeedLimit>> = HashMap::new();

    //Cache dijkstra
    for (name, level) in &nav.levels {
        for i in 0..level.vertices.len() {
            let v = &level.vertices[i];
            if v.2.is_parking_spot {
                parking_spots.insert(i);
            }
        }
        let graph: SimpleGraph<Position, SpeedLimit> =
            
            SimpleGraph::from_iters(level.vertices.iter().map(
            |p| Position::new(p.0.into(), p.1.into())), 
            level.lanes.iter().map(
                |e| (e.0, e.1, SpeedLimit(None))
            ));

        level_map.insert(name, graph);
    }

    let mut cached_dijkstra: HashMap<String, HashMap<usize, BTreeMap<OrderedFloat<f64>, usize>>> = HashMap::new();
    for (name, level) in &nav.levels {
        let mut cost_function: HashMap<usize, BTreeMap<OrderedFloat<f64>,usize>> = HashMap::new();

        let planner = Planner::new(Arc::new(AStar(InformedSearch::new_r2(
            SharedGraph::new(level_map[name].clone()),
            LineFollow::new(2.0).unwrap(),
        ))));
        for parking_spot in &parking_spots {
            for vert in 0..level.vertices.len() {
                if vert == *parking_spot {
                    continue;
                }
               
                let solution = planner.plan(0usize, 8usize).unwrap().solve().unwrap();
                match solution {
                    mapf::prelude::SearchStatus::Incomplete => continue,
                    mapf::prelude::SearchStatus::Impossible => continue,
                    mapf::prelude::SearchStatus::Solved(path) => {
                        let traj = path.make_trajectory::<WaypointR2>().unwrap().unwrap().trajectory;
                        let Some(last) = traj.into_iter().last() else {
                            continue;
                        };
                        let Some(first) = traj.into_iter().next() else {
                            continue;
                        };

                        let time = first.time - last.time;
                        let score = time.as_secs_f64();
                        let Some(ranked_preference) = cost_function.get_mut(&vert) else {
                            let mut tree_map = BTreeMap::new();
                            tree_map.insert(OrderedFloat(score), *parking_spot);
                            cost_function.insert(vert, tree_map);
                            continue;
                        };
                        ranked_preference.insert(OrderedFloat(score), *parking_spot);

                    },
                }
            }

        }
        
        cached_dijkstra.insert(name.clone(), cost_function);
    }

    let num_robots = parking_spots.len();

    //kuhn_munkres_min(weights)

    Ok(())
}
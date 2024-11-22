use chrono::Duration;
use std::{ops::Index, sync::atomic::AtomicBool};

use futures::pending;
/// This example show cases how to use a* for accounting for optimal parking spot allocation
/// The core idea is we have a set of robots that need to traverse an area. The goal is to pick up an item
/// and then have it delivered to a certain location.
///
/// Along the way we may place a door which serves as a bottle neck.
use pathfinding::prelude::astar;
use rmf_reservations::{
    algorithms::sat_flexible_time_model::{Problem, SATFlexibleTimeModel},
    cost_function::static_cost,
    database::DefaultUtcClock,
    ReservationParameters, ReservationRequestAlternative, StartTimeRange,
};

fn test_scenario() -> String {
    // Goal is for robot to transport i to I and j to J, with parking spots
    // "p" using robots 1 and 2.

    let room = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n
                     x.................x................x\n
                     x.1.....i.........x.........I......x\n
                     x.................D................x\n
                     x.......j.....pp..x..........2.....x\n
                     x.................x................x\n
                     x.................L.......J........x\n
                     x.................x................x\n
                     xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
                     ";
    room.to_string()
}

fn find_character_coordinate(map: &Vec<String>, character: &String) -> Option<(usize, usize)> {
    map.iter()
        .enumerate()
        .map(|(idx, f)| (f.find(character), idx))
        .fold(None, |num, res| {
            if res.0 == None {
                num
            } else {
                if let Some(row) = res.0 {
                    Some((res.1, row))
                } else {
                    num
                }
            }
        })
}

fn transform_to_map(map: String) -> Vec<String> {
    map.split("\n")
        .filter(|p| p.len() > 0)
        .map(|f| f.replace(" ", "").to_string())
        .collect()
}

fn display_path(path: &Vec<(usize, usize)>, m: &Vec<String>) {
    let mut m = m.clone();
    for p in path {
        m[p.0].replace_range(p.1..p.1 + 1, "*");
    }

    for line in m {
        println!("{:?}", line);
    }
}

fn astar_cost_estimate(
    start_goal: (usize, usize),
    end_goal: (usize, usize),
    m: &Vec<String>,
) -> Option<(Vec<(usize, usize)>, usize)> {
    let opts = vec![(0, 1), (1, 0), (0, -1), (-1, 0)];
    astar(
        &start_goal,
        |node| {
            let v: Vec<_> = opts
                .iter()
                .map(|(dx, dy)| (node.0 as i32 + dx.clone(), node.1 as i32 + dy.clone()))
                .filter(|(x, y)| {
                    *x >= 0 && *y >= 0 && *x < m.len() as i32 && *y < m[0].len() as i32
                })
                .filter(|(x, y)| m[*x as usize].bytes().nth(*y as usize).unwrap() != b'x')
                .collect();
            v.into_iter().map(|(x, y)| ((x as usize, y as usize), 1))
        },
        |p| p.0.abs_diff(end_goal.0) + p.1.abs_diff(end_goal.1),
        |f| *f == end_goal,
    )
}

fn ototot() {
    use std::sync::Arc;

    let current_time = chrono::Utc::now();

    let n = 60usize;
    let task_dur = Duration::seconds(100);
    let mut requests = vec![];
    for i in 0..n {
        requests.push(vec![ReservationRequestAlternative {
            parameters: ReservationParameters {
                resource_name: "Resource1".to_string(),
                duration: Some(task_dur),
                start_time: StartTimeRange {
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
    /*let model = SATFlexibleTimeModel {
        clock_source: DefaultUtcClock::default(),
    }
    .time_optimality_solver(&problem, stop);
    let result = model.unwrap();*/
}

fn main() {
    let m = transform_to_map(test_scenario());

    let Some(end_goal) = find_character_coordinate(&m, &"i".to_string()) else {
        panic!("couldnt find start");
    };
    let Some(start_goal) = find_character_coordinate(&m, &"I".to_string()) else {
        panic!("could not find end");
    };

    println!("{:?}", start_goal);
    println!("{:?}", end_goal);

    //let mut problem = Problem {}

    //display_path(&res.unwrap().0, &m);
}

use std::hint;
use std::{time::SystemTime, fs::OpenOptions};
use std::io::Write;

use fnv::FnvHashMap;
use rmf_reservations::algorithms::hierarchical_kuhn_munkres::{generate_test_scenario_with_known_best, TimeBasedBranchAndBound};

fn main() {
    for _ in 0..100 {
    let (requests, resources) =  generate_test_scenario_with_known_best(5, 5, 15);
    //println!("Requests {:?}", requests);
    
    let mut system = TimeBasedBranchAndBound::create_with_resources(&resources);
    for req in &requests {
        system.request_resources(req.clone());
    }
   // b.bench(|_| {
    let soln = system.generate_literals_and_remap_requests();
    soln.debug_print();

    let timer = SystemTime::now();

    if requests.len() < 2{
        continue;
    }

    let hint_length = requests.len() - 2;
    let hint = FnvHashMap::from_iter((0..hint_length).map(|k| (k, 0usize)));
    let res = soln.solve(hint);
    let dur = timer.elapsed();
    println!("Took {:?}", dur.clone().unwrap());

    println!("{:?}", res);

    let mut file = OpenOptions::new()
        .write(true)
        .append(true)
        .open("running_log.txt")
        .unwrap();

    writeln!(file, "{:?}, {:?}, {:?}", dur.unwrap(), requests.iter().flatten().count(), resources.len());
   }
       // Ok(())
}
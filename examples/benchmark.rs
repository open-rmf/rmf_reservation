use std::time::SystemTime;

use rmf_reservations::algorithms::hierarchical_kuhn_munkres::{generate_test_scenario_with_known_best, TimeBasedBranchAndBound};

fn main() {
    let (requests, resources) =  generate_test_scenario_with_known_best(6, 6, 15);
    //println!("Requests {:?}", requests);
    
    let mut system = TimeBasedBranchAndBound::create_with_resources(&resources);
    for req in requests {
        system.request_resources(req);
    }
   // b.bench(|_| {
    let soln = system.generate_literals_and_remap_requests();
    soln.debug_print();

    let timer = SystemTime::now();
    let res = soln.solve().unwrap();
    println!("Took {:?}", timer.elapsed());

    println!("{:?}", res);
       // Ok(())
}
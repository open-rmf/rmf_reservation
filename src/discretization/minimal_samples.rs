use std::collections::{HashSet, BTreeMap};

use chrono::{DateTime, Utc};

use crate::{utils::multimap::UniqueMultiHashMap, ReservationSchedule, Assignment};

use super::DescretizationStrategy;

use std::hash::Hash;

/// A discretization policy that tries to find all insertion points assuming that the cost
/// of insertion is linearly dependent on the time. Note that this is n! in the number of alternatives
/// it may generate (particularly if no constraints are placed on timing). If you have more than
/// 10 items in your schedule and no constraints, it is highly recommended
/// not to use this. On the other hand if there are only a few free reservations, or your reservations
/// have a large number of constraints feel free to use it.
 
pub struct MinimalSamples {
    earliest_start: DateTime<Utc>
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Trail {
    order: ReservationSchedule,
    explored_options: HashSet<(usize, usize)>,
}

impl Hash for Trail {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.order.hash(state);
    }
}

/*fn find_options_within_resource(
    earliest_start: DateTime<Utc>,
    requests: &Vec<Vec<crate::ReservationRequest>>,
    resource_maps: &HashSet<(usize, usize)>) -> UniqueMultiHashMap<(usize, usize), DateTime<Utc>> {
    
    //let mut explored = HashSet::new();
    let mut stack = vec![];
    let mut result = UniqueMultiHashMap::new();

    for &(req_id, alt_id) in resource_maps {
        let request = requests[req_id][alt_id];
        let earliest = request.parameters.start_time.earliest_start.unwrap_or(earliest_start);
        stack.push(Trail {
            order: ReservationSchedule { 
                schedule: BTreeMap::from_iter([((earliest), Assignment(req_id, alt_id, request.parameters.duration.clone()))]) 
            },
            explored_options: HashSet::from_iter([(req_id, alt_id)])
        });
    }

    while let Some(schedule) = stack.pop() {
        let mut num_added = 0;
        for &(req_id, alt_id) in resource_maps {
            if schedule.explored_options.contains(&(req_id, alt_id)) {
                continue;
            }

            let mut new_sched = schedule.clone();
            
            let Some((time, assignment)) = new_sched.order.schedule.last_key_value() else {
                continue;
            };

            let Some(duration) = assignment.2 else {
                continue;
            };

            let potential_start_time = *time + duration;


           // new_sched.order.schedule.insert(key, value);
            new_sched.explored_options.insert((req_id, alt_id));
            num_added += 1;
        }

        if num_added == 0 {
            // This is a final solution
            //result.insert(key, value)
        }
    }
    result
}*/

impl DescretizationStrategy for MinimalSamples {
    fn discretize(&mut self, requests: &Vec<Vec<crate::ReservationRequest>>) -> Vec<Vec<crate::ReservationRequest>> {
        let mut resource_assignment_mapper = UniqueMultiHashMap::new();
        for req_id in 0..requests.len() {
            for res_id in 0..requests[req_id].len() {
                resource_assignment_mapper.insert(
                    requests[req_id][res_id].parameters.resource_name.clone(), (req_id, res_id));
            }
        }

        for (resource, alternatives) in resource_assignment_mapper.iter() {

        }

        vec![]
    }

    fn remap(&self, ticket_id: &(usize, usize)) -> (usize, usize) {
        todo!()
    }
}

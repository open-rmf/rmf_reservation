# Using the SAT solver for Simple constraint scheduling tasks

We provide two formulations for the SAT solver based approach. First, we have an approach which uses fixed timestamps. Suppose you have a scenario where 5 robots want to access 2 chargers. The robots must use the chargers within the next 5 hours. It takes 2 hours to charge on each charger. We can write each robot's request as:
```rust
let chargers = ["Charger 1".to_string(), "Charger 2".to_string()];
let start_time = Utc:now();
// Since we said it needs to be done in a time window of 5 hours and it takes 2 hours to charge therefore the latest start time is in 3 hours
let latest_start_time = start_time + Duration::hours(3); 


/// First robot alternative
let robot1_alternatives = vec![
    ReservationRequestAlternative {
        parameters: ReservationParameters {
            resource_name: chargers[0].clone(),
            duration: Some(Duration::minutes(120)),
            start_time: StartTimeRange {
                earliest_start: Some(start_time),
                latest_start: Some(latest_start_time),
            },
        },
        cost_function: Arc::new(StaticCost::new(1.0)),
    },
    ReservationRequestAlternative {
        parameters: ReservationParameters {
            resource_name: chargers[1].clone(),
            duration: Some(Duration::minutes(120)),
            start_time: StartTimeRange {
                earliest_start: Some(start_time),
                latest_start: Some(latest_start_time),
            },
        },
        cost_function: Arc::new(StaticCost::new(1.0)),
    },
];

/// We can add more robots
let robot2_alternatives = robot1_alternatives.clone();
let robot3_alternatives = robot1_alternatives.clone();
let robot4_alternatives = robot1_alternatives.clone();
let robot5_alternatives = robot1_alternatives.clone();
```
Lets set up the SAT based solver problem:
```rust
let problem = Problem {
        requests: vec![req1],
    };
```

We are going to use the constrained SAT solver to see if we can get a feasible schedule
```rust
let model = SATFlexibleTimeModel {
                clock_source: FakeClock::default(),
            };
```
A clock source is needed because in the event no starting time is provided, we need to know the current time.
We can then check if it is even feasible to schedule 5 robots like this on two chargers.
```rust
// The stop signal exists so other threads can tell the solver to cleanly exit. For the single
// threaded case we can ignore it.
let stop = Arc::new(AtomicBool::new(false));
let result = model.feasibility_analysis(&problem, stop);
```
Note: It is much faster to check feasibility then optimize later on.
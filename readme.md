# RMF Reservations

This is a library that provides a solver for constrained resource scheduling for multi-robot applications. More specifically,
we provide a very simple formulation for resource optimization and scheduling. A robot may request the use of a
resource like a charger for a fixed duration of time within a given time range. The system will then assign the robot
to said resource.

At 10,000ft the library solves the following problem:

> Suppose you have n robots declaring that “I'd like to use one of (resource 1, resource 2, resource 3) for (d1, d2, d3) minutes starting at a certain time. Each alternative has some cost c(t).”

This is the type of resource constraint scheduling that this library can solve:
```
Robot1 Charging Request:

Alternatives:
    - Charger 1
        - Start Time Range From:10:00 to 11:00
        - Duration 1 hour
        - Cost: My own cost function
    - Charger 2
        - Start Time Range From:10:00 to 12:00
        - Duration 1 hour
        - Cost: My own cost function

Robot2 Charging Request:

Alternatives:
    - Charger 2
        - Start Time Range From:10:00 to 11:00
        - Duration 1 hour
        - Cost: My own cost function
    - Charger 3
        - Start Time Range From:10:00 to 13:00
        - Duration 1 hour
        - Cost: My own cost function
```

The requests can come in asynchronously. We can solve both optimally and sub-optimally depending on the complexity of the problem.

A variety of algorithms have been implemented in this library including SAT based algorithms and greedy algorithms.

For more details take a look at the tutorial:


## Publications

If you use this work in your research please cite:

```
Arjo Chakravarty, Michael Grey, Viraj Jagathpriya Muthugala Muthugala Arachchige* and Mohan Rajesh Elara "From Concept to Field Tests: Accelerated Development of Multi-AUV Missions Using a High-Fidelity Faster-than-Real-Time Simulator," in IEEE/RSJ International Conference on Intelligent Robots and Systems (IROS), Oct 2024.
```

## Roadmap

* [ ] Cancel reservations
* [ ] Extend reservations
* [ ] Block off resource for a given time

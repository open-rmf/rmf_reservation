#!/bin/bash
cargo build --release --example lift_session
for ((i=0;i<20;i++)); do
    timeout 300 ./target/release/examples/lift_session
    export COPY_DIR=data/15robot_6lift_OPT_LOW_TRAVEL$(date +%s%N)
    mkdir $COPY_DIR
    mv perf.txt $COPY_DIR/
    mv perf.teg.txt $COPY_DIR/
    mv time.txt $COPY_DIR/
    mv time.teg.txt $COPY_DIR/
    mv greedy.time.txt $COPY_DIR/
done
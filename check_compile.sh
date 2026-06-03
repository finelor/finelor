#!/bin/bash
cd /home/hamed/projects/finelor
cargo check --features ssr 2>&1 | head -200

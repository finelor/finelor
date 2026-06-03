#!/bin/env bash
cd /home/hamed/projects/finelor
# Test cargo check
echo "=== Testing cargo check ==="
rustc --version
cargo --version
cargo check --features ssr -p finelor 2>&1

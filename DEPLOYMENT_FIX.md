# Finelor Dev Deployment Fix

## Problem: wasm-bindgen version mismatch

The build was failing because:
- Cargo.lock has `wasm-bindgen = 0.2.122`
- Dockerfile.dev had `WASM_BINDGEN_VERSION=0.2.118`

## Fix Applied

Updated both Dockerfiles:
- docker/Dockerfile.dev: WASM_BINDGEN_VERSION=0.2.122
- docker/Dockerfile.release: WASM_BINDGEN_VERSION=0.2.122

## Next Steps

To deploy with make dev-up:

1. Clean Docker build cache (important!):
   ```bash
   docker buildx prune -f
   ```

2. Or use --no-cache flag by modifying Makefile temporarily

3. Then run:
   ```bash
   make dev-up
   ```

## Alternative: Local Development

Since Docker has these version issues, you can also run locally:

```bash
# Install dependencies
make bootstrap-local

# Create env
make env-init

# Run locally
make run
```

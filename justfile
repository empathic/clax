set shell := ["bash", "-cu"]

default: build

build:
    cargo build --workspace

web-install:
    cd web && npm ci

web: web-install
    rm -rf web/dist/_artifax web/dist/index.html
    cd web && npm run build

web-test: web-install
    cd web && npm run typecheck && npm test

web-e2e: web
    cd web && npx playwright install --with-deps chromium && npm run e2e

test:
    cargo test --workspace

ci:
    scripts/quality_gates.sh

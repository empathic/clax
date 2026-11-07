# The image the demo's containers start from: node for the sample app's
# tests, and git. `safe.directory` because the repository is a read-only
# mount owned by another user.
FROM mcr.microsoft.com/playwright:v1.59.1-noble
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates && rm -rf /var/lib/apt/lists/*
RUN git config --system --add safe.directory '*'

# Postman collection (dev only)

`veriface.postman_collection.json` covers every endpoint in `contracts/openapi.yaml`, with sample requests on the public-domain `testdata/`.

1. `node tools/console/scripts/dev-setup.mjs` creates a dev key and `tools/console/.dev/veriface.local.postman_environment.json` is generated next to it (git-ignored; holds the key).
2. Start the service with `tools/console/.dev/config.toml`.
3. Import the collection and the environment into Postman and select the environment.

File paths in the sample bodies are absolute paths on the machine that generated them; re-select files if you move the repo.

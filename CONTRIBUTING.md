# Contributing to A-Eye

Start with the [product definition](docs/PRODUCT.md), [roadmap](docs/planning/ROADMAP.md), and [architecture](docs/architecture/SYSTEM.md). The repository is a planning foundation, not a functioning desktop app.

For a first contribution, take one Phase 0 spike and return a reproducible result with exact versions, synthetic evidence, limitations, and a decision. Do not create a large speculative scaffold to make the tree look finished.

Use short-lived branches: `spike/`, `feat/`, `fix/`, or `docs/`. Keep `main` coherent. Open focused PRs explaining the problem, resulting behavior, verification, and remaining limitations. Follow the existing MIT license; check third-party code/model/voice licensing separately.

Run `python scripts/check_docs.py` for documentation changes. Future implementation PRs must add relevant boundary tests and native acceptance evidence where needed. Do not claim a CI webview test validates Windows screen capture.

Use synthetic fixture applications and invented content. Never commit real private screenshots, transcripts, credentials, provider session dumps, or proprietary project files. Follow [SECURITY.md](SECURITY.md) for sensitive reports.

Visual contributions should implement the [experience specification](docs/design/EXPERIENCE.md) and include idle, listening, watching, busy, approval, and failure states. Accessibility and honest state communication are part of the design, not a final polish pass.

# Kiled — notes for agents

- `DESIGN.md` is the spec; `CONTRIBUTING.md` has the checks and conventions.
- Kiled is Kil's sibling (`../kil`): same stack, same conventions, same release flow. Keep it
  a code editor and nothing else.
- Work and commit directly on `main`; don't create branches.
- **To release** ("发版", "new patch version", "打包"), follow [`RELEASING.md`](RELEASING.md)
  step by step: bump, changelog, README, `scripts/release.sh`, install into
  `/Applications`, commit `Kiled x.y.z: …`, push.

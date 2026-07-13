# roomrs-cli

Command-line tools for [roomrs](https://github.com/yongaru/roomrs) schema snapshots and migration drafts.

## Installation

```text
cargo install roomrs-cli
```

## Commands

```text
roomrs migrate diff <old.json> <new.json> [out.sql]
roomrs migrate check <a.json> <b.json>
roomrs migrate check-dir <schema_dir> <db_name> [--strict]
```

See the [repository README](https://github.com/yongaru/roomrs#readme) for snapshot workflow and review guidance.

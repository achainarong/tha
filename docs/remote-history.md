# Remote performance history

The persistent `tha-agent.service` collects metrics for all running containers on
each configured engine, independently of SSH connections and the selected view.
No extra network port, Docker CLI, or Security module is required for collection.

- **Sampling:** every 1 second; container inventory refreshes every 5 seconds.
- **Persistence:** the same bounded writer as the local daemon, flushing every
  15 seconds, early when the buffer fills, and on clean shutdown. Abrupt termination
  can lose the pending samples.
- **Storage:** `/var/lib/tha-agent/history-<engine>.sqlite`, private to the agent.
  Each engine has its own database and a 256 MiB database/WAL budget.
- **Retention:** 24 hours with incremental pruning. A full disk budget pauses new
  persistence rather than removing recent history, and the chart reports a warning.
- **Queries:** read-only aggregates for one immutable container ID and a bounded
  time range, over the existing authenticated SSH relay. Unflushed samples are
  merged without double counting, so live charts do not wait for a SQLite flush.

History survives reconnecting Tha and restarting the agent. Collection starts
after installing this version; earlier readings cannot be reconstructed. Missing
observations stay as gaps rather than being replaced with zeroes.

Deploy the updated client and agent, then restart `tha-agent.service` on the server
(the agent installer also restarts it). No policy migration is required:

```sh
sudo systemctl restart tha-agent
sudo journalctl -u tha-agent
```

Older agents continue to provide live session metrics at their configured polling
interval (5 seconds by default). Tha displays an upgrade/restart notice instead of
claiming persistent history is available. Server/query failures also show a notice
and leave session metrics usable. The chart uses the selected source's sampling
interval, not the local daemon's interval.

# macOS host, credentials from files

**What.** `serve` runs under launchd on macOS. The worker reads each credential from its own file, on Linux and macOS.

**Why.**

- README.md:9 claims "Linux or macOS". Only a systemd unit ships: `systemd/scswarm@.service`.
- The unit passes credentials by `EnvironmentFile=` (systemd/scswarm@.service:10). launchd has no equivalent.
- Contract.md:272: a refreshed credential needs a `serve` restart, since values come from the `serve` environment.
- systemd.exec(5), `LoadCredential=`: environment variables reach every child process and are unsafe for secrets. Docker secrets, Kubernetes secret volumes and systemd all use one file per secret, name = file name.
- Measured on darwin 24.6.0, podman 6.1.4, rootless podman machine, 2026-10-26:
  - The VM shares `/Users`, `/private`, `/var/folders` over virtiofs, label `nfs_t`. `~/swarm` sits under `/Users`.
  - A bind with `rw,z` and `--userns=keep-id` runs as uid 501. Files the container writes read as the host user on the host.
  - A bind with `ro,z` refuses writes: `Read-only file system`.
  - `--env=NAME` carries the client value into `run` and `exec`.
  - `make e2e` passes with no code change: 1 passed, 120.69 s.
  - `which podman`: `/opt/homebrew/bin/podman`. launchd starts agents with `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, which misses it.

**Done when.** On macOS under the shipped launchd agent, a row whose host `env` names `ANTHROPIC_API_KEY` runs its agent step with the value from `~/.config/scswarm/credentials/ANTHROPIC_API_KEY`, and `make e2e` passes on Linux and macOS.

## Design

### Credentials

- Directory `~/.config/scswarm/credentials/`, under `XDG_CONFIG_HOME` when set. One file per credential: file name = variable name, contents = value.
- The directory must grant no group or other bits, mode `0700`. The rule of `~/.ssh`.
- One trailing `\n` strips, so `echo key > file` works. Other bytes pass as is.
- Sole source. The `serve` and worker environments carry no credential.
- The worker reads the names of its job at row start. A refresh applies from the next row, with no restart.
- Missing directory, wrong mode, missing file: machine fault, exit 2, no fail mark. The error names the directory or file and the expected mode, never the value.

File by file:

| File | Change |
|---|---|
| `src/paths.rs` | `credentials()`: the directory path |
| `src/credential.rs` | new. `load(names) -> Result<Vec<(String, OsString)>>`: mode check, read, strip. Sole reader of the directory |
| `src/work.rs` | `attempt` loads the job's names before `Session::open`, passes the pairs down |
| `src/call.rs` | `env: &[String]` becomes the loaded pairs |
| `src/podman.rs` | `exec` sets `Command::env(name, value)`, keeps `--env=NAME`. `present` goes: `credential::load` checks presence |
| `systemd/scswarm@.service` | `EnvironmentFile=` goes |
| `docs/spec/contract.md` | host `env` names credentials; credentials section; limit 272 goes |
| `README.md` | step 5 writes credential files, not exports |

Host field `env` keeps its name: it names variables inside the container.

### launchd agent

`launchd/com.softcery.scswarm.plist`, a template. launchd has no instances and expands no `~`, so the user fills `PROJECT` and `HOME` and installs one copy per project as `~/Library/LaunchAgents/com.softcery.scswarm.PROJECT.plist`.

| Key | Value | systemd peer |
|---|---|---|
| `Label` | `com.softcery.scswarm.PROJECT` | `scswarm@PROJECT` |
| `ProgramArguments` | `HOME/.cargo/bin/scswarm`, `serve`, `PROJECT` | `ExecStart` |
| `EnvironmentVariables` | `PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin` | user `PATH` |
| `RunAtLoad`, `KeepAlive` | `true` | `Restart=always`, `WantedBy=default.target` |
| `AbandonProcessGroup` | `true` | `KillMode=process` |
| `StandardOutPath`, `StandardErrorPath` | `HOME/Library/Logs/scswarm/PROJECT.log` | journal |

README section "Run under launchd": install, `launchctl bootstrap gui/$(id -u) <plist>`, upgrade with `launchctl kickstart -k gui/$(id -u)/com.softcery.scswarm.PROJECT`, stop with `launchctl bootout`.

### Docs

- README.md:9, CONTRIBUTING.md:7 and :22, CLAUDE.md "Tools", Makefile:15, tests/e2e.rs:25: e2e runs on Linux or macOS.
- Contract and architecture limits gain the macOS limits below.
- Contract.md:212 and README.md:264: "run under systemd or launchd".

## Steps

1. Credentials: `paths`, `credential`, `work`, `call`, `podman`, systemd unit, contract, README.
2. launchd template and README section.
3. Doc limits and e2e wording.
4. `make lint`, `cargo test`, `make e2e` on Linux and on macOS.
5. On macOS: check each unverified fact below under `launchctl`.

## Notes

- Breaking: setups that pass credentials by environment stop working. The implementing commit takes `!` and a `BREAKING CHANGE:` footer.
- A terminal `serve` needs the credential files too.
- `devices` must stay empty on macOS. Device nodes live in the VM; the host check in `src/device.rs` fails each entry.
- A Mac asleep pauses the VM. Steps in flight can hit `timeout` and spend tries.
- The podman machine does not start at login. Until it runs, `serve` exits 2 and launchd restarts it every 10 s, the default `ThrottleInterval`.
- `mounts` host sides must sit under a VM share: `/Users`, `/private`, `/var/folders`.
- Hard stop under launchd: `pkill -f 'scswarm work PROJECT'`. launchd has no command that kills an abandoned process group.
- The log file never rotates. launchd holds it open.
- Unverified:
  - `AbandonProcessGroup=true` keeps workers alive through `kickstart -k` and `bootout`.
  - `KeepAlive=true` restarts `serve` after exit 2.
  - `serve` recovers once the VM comes up after login.

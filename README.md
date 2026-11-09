# cluster

A terminal UI that runs a batch of problem instances on a set of idle machines over ssh.

It shows which hosts are busy or idle, lets you pick a subset of the idle ones, and runs a command on every file of an instances directory. Each selected host runs one instance at a time; as soon as a run ends, the host gets the next file, until all instances are done. The standard output of every run is collected in one text file.

## Requirements

- Rust (stable) to build it
- `ssh` and `scp` on your `PATH`, with key-based login to every host (no password prompts)

## Build and install

```
cargo build --release          # binary: target/release/cluster
cp target/release/cluster ~/.local/bin/
```

or `cargo install --path .` (installs into `~/.cargo/bin`).

## Hosts file

One IP address (IPv4 or IPv6) per line. Blank lines and lines starting with `#` are ignored.

```
192.168.81.10
192.168.81.11
# 192.168.81.12 is being repaired
192.168.81.13
```

The file is validated at startup. If an entry is not a valid IP address or appears twice, the program prints every offending line and exits with an error.

By default `./hosts.txt` is used. If it is missing and `--hosts` is not given, the program exits with an explicit message.

## Usage

```
cluster [--hosts FILE] [--user USER] [--idle-threshold LOAD] [--append] [--results-dir DIR]
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--hosts FILE` | `./hosts.txt` | File with the host addresses |
| `--user USER` | from `~/.ssh/config`, else current user | ssh user |
| `--idle-threshold LOAD` | `0.5` | A host is idle if its 1-minute load average is below this value. The load average is not divided by the number of cores, so `0.5` means one core is less than 50% busy |
| `--append` | off (overwrite) | Add to the end of an existing results file instead of overwriting it |
| `--results-dir DIR` | `results` | Base directory for results |

### In the interface

The screen has, from top to bottom: the **Hosts** table, the instances directory and command fields side by side, and a progress bar. During a run, each selected host shows what it is running (instance name and elapsed seconds) on its own row of the Hosts table. If there are more hosts than fit on screen, the table scrolls with the cursor.

1. In the host table, select the idle hosts you want to use.
2. Type the instances directory.
3. Type the command: the executable followed by its flags, for example `./bottom_left --time 60`. The instance path is appended as the last argument, so the command run on the host is `./bottom_left --time 60 <instance>`. The executable path must not contain spaces.
4. Press `Enter` to start.

| Key | Where | Action |
| --- | --- | --- |
| `Tab` / `Shift-Tab` | anywhere | Move focus between table, directory and command |
| `Enter` | anywhere | Start the run |
| `↑` / `↓`, `PgUp` / `PgDn` | host table (also during a run) | Move the cursor and scroll |
| `space` | host table | Select an idle host, or deselect a host |
| `a` | host table | Select all idle hosts |
| `r` | host table | Probe all hosts again |
| `q` | host table | Quit |
| `q`, then `y` | during a run | Abort: kills the remote runs and removes the remote directories |
| `Ctrl-C` | anywhere | Quit immediately |

Host states: `IDLE`, `BUSY`, `DOWN` (unreachable, cannot be selected), and, during a run, `RUN` (with the current instance), `DONE` (no more work for that host) and `FAIL` (with the error). Hosts keep being probed during a run, so the other hosts stay up to date and you can start another batch as soon as a run ends.

## How it works

- **Idle detection:** every 5 seconds each host is probed over ssh (`/proc/loadavg` and the top CPU process). A 1-minute load average below the threshold means idle.
- **Files:** the executable is copied once to `/tmp/cluster-<pid>/` on each selected host. For every instance, the file is copied there, the command runs, and the file is deleted. The remote directory is removed at the end.
- **Instances:** the regular files directly inside the directory, sorted by name. Subdirectories and files starting with `.` are ignored, and the search is not recursive.
- **Failures:** if a host fails at the ssh level (connection lost), its current instance goes back in the queue for another host and that host is dropped from the run. A command that exits with a non-zero status is not retried: it is counted in the progress bar as a non-zero exit and its output is still recorded.
- **Host keys:** unknown host keys are added to `known_hosts` automatically; a changed key is refused. If a host reports "Host key verification failed", remove the old entry with `ssh-keygen -R <ip>`.

## Results

Results are written to

```
<results-dir>/<instances path>/<exe>.txt
```

where `<instances path>` is the instances directory as typed (only normal path components are kept, so `./`, `..` and a leading `/` are dropped) and `<exe>` is the file name of the executable. For example, instances in `data/test` and the executable `./bottom_left` give `results/data/test/bottom_left.txt`.

Each finished run adds a block: a `# <instance>` header line followed by the standard output of the command, exactly as it printed it:

```
# 012379a0-f981-4053-9144-29d2d31ae611
{"obj":94.0,"bound":49.0,"runtime":0.000023975}
# 3f1c2b7e-0d5a-4f55-8a31-9c2b1d7e4a10
iteration 1: 120
iteration 2: 97
done
```

- `<instance>` is the file name without its extension (`012379a0.txt` becomes `012379a0`; only the last extension is removed). If two instances would get the same name once the extension is removed (for example `a.txt` and `a.json`), the run does not start and the error names the clashing name.
- Blocks appear in the order the runs finish. The file is only ever added to, so it is complete up to the last finished run even if you abort.
- A run that exits non-zero still gets its block, with whatever it printed before. An instance with no output has just its header line.
- stderr, exit codes, hosts and timing are not stored.
- The file is overwritten on each run unless `--append` is given. With `--append` new blocks go after the existing content, so an instance that is run again appears twice.
- The format cannot tell a header from an output line that itself starts with `# `.

## Development

```
cargo test
cargo fmt      # formatting settings are in rustfmt.toml
```

Code layout:

| File | Contents |
| --- | --- |
| `src/main.rs` | Command-line options and the terminal event loop |
| `src/app.rs` | Application state, key handling, events from background tasks |
| `src/app/ui.rs` | Drawing |
| `src/hosts.rs` | Hosts file, ssh/scp wrapper, idle probe |
| `src/scheduler.rs` | Batch runs: instance queue, one worker per host |
| `src/results.rs` | Results file |

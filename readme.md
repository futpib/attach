# attach

A command-line tool for managing and attaching to terminals running inside Docker containers and tmux sessions.

## Installation

```sh
cargo install --path .
```

## Usage

```
attach [TARGET_URL]
attach <COMMAND>
```

When called with a target URL and no subcommand, `attach` connects to that target directly.  
When called with no arguments, `attach ls` is run implicitly.

### Commands

| Command | Description |
|---|---|
| `ls` | List all attachable targets |
| `attach <target>` | Attach to a target |
| `screenshot <target>` | Print one frame of the target's terminal output |

### Target URLs

Targets are identified by URLs of the form `scheme://path`.

| Scheme | Format | Example |
|---|---|---|
| Docker container | `docker://<name>` | `docker://my-container` |
| Docker Compose service | `docker://<project>/<service>` | `docker://myapp/web` |
| tmux pane | `tmux://<session>/<window>/<pane>` | `tmux://main/0/1` |

### Examples

List all available targets:

```sh
attach
# or
attach ls
```

Attach to a Docker container:

```sh
attach docker://my-container
attach docker://myapp/web
```

Attach to a tmux pane:

```sh
attach tmux://main/0/1
```

Take a screenshot of a target's current terminal output:

```sh
attach screenshot docker://my-container
attach screenshot tmux://main/0/1
```

## Dependencies

- [Docker](https://docs.docker.com/get-docker/) – required for `docker://` targets
- [tmux](https://github.com/tmux/tmux) – required for `tmux://` targets

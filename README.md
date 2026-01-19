# Shepard

A terminal multiplexer for running multiple Claude Code sessions concurrently.

## Overview

Shepard lets you manage multiple Claude Code instances from a single terminal. Switch between sessions instantly, run a shell alongside Claude, and use git worktrees to keep each session's work isolated in its own branch.

## Installation

```bash
cargo install --path .
```


## Usage

Run `shepard` from any git repository:

```bash
cd your-project
shepard
```

## Requirements
- Claude Code
- Rust 


## Configuration

Configuration is stored at `~/.shepard/config.json`:

## License 

MIT

--- 
> Good times create weak men

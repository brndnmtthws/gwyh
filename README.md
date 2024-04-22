[![Docs](https://docs.rs/gwyh/badge.svg)](https://docs.rs/gwyh) [![Crates.io](https://img.shields.io/crates/v/gwyh)](https://crates.io/crates/gwyh) [![Build & test](https://github.com/brndnmtthws/gwyh/actions/workflows/build-and-test.yml/badge.svg)](https://github.com/brndnmtthws/gwyh/actions/workflows/build-and-test.yml) [![Codecov](https://img.shields.io/codecov/c/github/brndnmtthws/gwyh)](https://app.codecov.io/gh/brndnmtthws/gwyh/)

# gwyh: Gossip Wit Your Homies 💖 ✨

![Homies gossiping](homies.png)

Gwyh (pronounced _gwyh_) is a library for building gossip-based services in Rust. Gwyh has some unique features that make it lightning quick and especially cool:

* UDP-based
* Built-in congestion control
* Super quick dissemination (broadcasts always take quickest path)
* 100% async, based on [Tokio](https://tokio.rs/)
* Built-in transport layer encryption using super-fast XSalsa20 via [dryoc](https://crates.io/crates/dryoc)
* Effortlessly scales to thousands of nodes without compromising speed
* Zonal awareness, with several distribution strategies to choose from

Gwyh can provide the gossip protocol layer for your service, but gwyh is not
appropriate for all uses. Messages broadcasted with gwyh aren't acknowledged,
and there's no guarantee that broadcasts will be delivered. Gwyh's speed comes
with the trade-off of ignoring a lot of best practices regarding packet delivery
over unreliable or semi-reliable networks (mainly from the lack of delivery
acknowledgement).

Do not be deterred, however, because gwyh is a great fit for eventually
consistent systems provided your own protocol is idempotent and can tolerate the
occasional dropped message.

Gwyh was designed to work well with globally distributed networks where latency
is variable and the fastest path is difficult to estimate. Gwyh's wire protocol
uses strong encryption that allows you to even use it over the Internet without
requiring additional layers (like a VPN) if you choose, although practically
speaking this may not work because each gwyh instance must be reachable (i.e.,
it won't work behind a NAT or load balancer).

Gwyh is optimized for sending small messages quickly. In the context of gwyh, a
small message is one which fits inside a single UDP pocket (a bit less than
64KiB).  Gwyh _can_ handle messages that are multiple packets in length, but
doing so incurs some additional overhead. Building a stream protocol, for
example, on top of gwyh is not the best use of it.

## Protocol

See [docs/PROTOCOL.md](docs/PROTOCOL.md) for details on gwyh's underlying protocol.

## Examples

_TODO: add examples here_

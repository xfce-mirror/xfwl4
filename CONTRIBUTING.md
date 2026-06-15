# Contributing to xfwl4

Contributions are welcome!  Below you'll find some guidelines on how to
best contribute to the project.

## Issues

If you've found a problem with xfwl4, please let us know!  Keep the
following things in mind:

* Search the issue tracker before filing a new issue.  Someone may have
  already filed one for the problem you've found.
* There is an issue template that will show up in the description box
  when you start to create a new issue.  Fill *every* part of it out,
  not just the parts you *think* are relevant.  Issues with incomplete
  templates may be closed without comment.
* If you are not running the latest version of xfwl4 and any relevant
  dependencies, we may not be able to take the time to look into your
  issue until you've updated.
* Do not use an AI/LLM tool to write your issue description.  In all
  cases I've seen, such issues are way too verbose, and contain a lot of
  extraneous information that makes it really hard to understand and do
  anything with.  If you're not a native English speaker and need help
  with that aspect, you're better off writing in your native language,
  pasting it into a translation tool such as Google Translate, and
  pasting the English output into the issue description.  Issues that
  appear to be LLM-generated may be closed without comment.
* The issue tracker is not a support forum.  If you need help setting up
  or using the software, or have other general questions, ask on our
  [Matrix channel](https://matrix.to/#/#xfce:matrix.org).

If you're ready to search the issue tracker and possibly file an issue,
here's where you'll want to go:
https://gitlab.xfce.org/xfce/xfwl4/-/issues

Note that xfwl4 aims to provide as close as possible to the exact same
user experience as the Xfce desktop environment running on X11, using
xfwm4 as the window manager.  If there are missing features or behaviors
that don't match, those absolutely do count as issues to be filed.

## Patches / Merge Requests

If you've found a problem and have decided to try to solve it yourself,
great!  Before you do so, check on the issue tracker to see if there's
already an issue for it, which may have some discussion that will be
useful.  If there isn't already an issue, open one, and explain what's
wrong and how you plan to solve it.

At this point in xfwl4's development lifecycle, let's talk about the
issue you're having before you write any code, because we may have
guidance specific to the particular feature you are trying to implement
or bug you are trying to fix.  That discussion can happen on the issue
tracker, or in our [Matrix
channel](https://matrix.to/#/#xfce:matrix.org).  If you submit a merge
request without discussing things with us beforehand, don't be surprised
if there's some pushback on the design of your change or on the feature
itself.

For now, focus on issues with behavior and feature parity with Xfce on
X11.  New functionality beyond that will likely be rejected at this
time.

### Development Environment

After cloning the repository, run `make hooks` before doing anything
else.  This will set up a pre-commit hook that will do some checks on
the code you're about to commit.  The hook can take a bit of time to
run, so feel free to commit with `--no-verify` until you are ready to
open the merge request.  The same hooks run on our Gitlab server when a
MR is created or updated, and will fail the build if they don't pass, so
save yourself some time by running them locally.

For reference, the following are the checks that must pass:

* Formatting: just run `cargo fmt`.
* Linting: run `cargo clippy -- -D warnings`.  Any warnings will fail
  the build.
* Licenses: `cargo deny check licenses` ensures that all dependencies
  have compatible licenses.
* Advisories: `cargo deny check advisories` ensures that no dependencies
  have security vulnerabilities.

### Guidelines

xfwl4's minimum Rust version is 1.90.0, so ensure you don't use language
features or APIs stabilized after that.

Avoid adding more dependencies if you can help it.  If there's
functionality you need that is relatively small, implement it yourself
before pulling in a new dependency.  It's fine to promote a transitive
dependency to a direct dependency if there's something useful you want
to use.

Prefer iterators and functional-style code over imperative code.  Don't
mutate variables and data unless it's significantly more readable to do
so.  Write pure functions when possible and appropriate, and for any
non-trivial logic, include a unit test.

Factor out repeated code when it's longer than a few lines, or is
repeated many times (this can take the form of a helper function, or a
local closure, depending on what's clearer).

Comments should be infrequent, and should note *why* you have done
something that is perhaps not obvious.  Comments should never be used to
describe *what* the code is doing; if that isn't clear from the code
itself, rewrite the code until it is.

Don't write gratuitous unit tests.  Unit tests should be added for
tricky functionality or to validate math or other similar behavior.
Every unit test is also code that has to be maintained, and is extra
time that we have to wait for the build to run.

MRs must be warning-free (both compiler and clippy), and formatted with
`rustfmt`.

### Structure

xfwl4 is based around the `calloop` event loop library.  A big
limitation that imposes is that event loop callbacks are all passed the
same bit of mutable data: `&mut Xfwl4State`.  `Xfwl4State` is broken up
into a backend instance (something implementing the `Backend` trait),
and `Xfwl4Core`, which contains more or less everything else.

Because of this limitation in the event loop, the code is not as divided
into modules/interfaces as well as I'd like, but here are a few hints:

* The backends (`src/backend/`) should not call into the core
  (`src/core/`), except for in a few small where it already does (and I
  would like to reduce that surface as well in the future). There are
  already many functions on `Xfwl4State` and `Xfwl4Core` that are public
  to the crate (or public in general) that should be more narrowly
  scoped.  Any new call from the backends into the core needs
  justification.
* The UI (`src/ui/`) should not know anything about the backends or
  core.  Think of it as an entirely independent thing that only knows
  what can see from the `xfwl4-compositor-ui-v1` protocol.
* Custom protocol code (`/src/protocols/`) should be standalone and
  generic, and not rely on or know anything about xfwl4's internals (the
  idea is that they could in theory be upstreamed to smithay or used in
  another compositor).  The right place for the xfwl4-specific code is
  in the handlers (`src/core/handlers/`).

The core itself can be a bit all over the place, since there aren't
great module boundaries.  This will slowly be tightened up over time,
but as you are making changes, try to keep things isolated and
encapsulated wherever you can.

### AI/LLM Use

xfwl4 does not currently accept merge requests that were entirely
generated using an LLM or other "AI" agent.  Submitted code should be
your own work.  You can use an LLM to help with verification and solving
issues with your own code, or for writing unit or client tests (most of
the existing tests in xfwl4 were written by an LLM), but we are not
interested in reviewing LLM-generated output.  If/when we want that, we
can do it ourselves, and have a lot more context about the code and will
be better able to guide the LLM.

One exception: trivial fixes of a handful  of lines are ok if an LLM finds
and fixes the problem.  Do make sure the LLM follows our guidelines, and
trim any verbose comments the LLM adds, because that's what they always
do.  In this case you *must* disclose that the MR was written by an LLM.

# Capture prime residency

Each production capture command receives the memory capability of its existing
`FrameStore`. Host creates the pure-memory store before the command and moves that
same store into `CapturePipeline` at the material boundary. Opening the pipeline
still emits its policy event at that boundary. The memory capability carries no
lease, fencing witness, backend reference or persistent state.

`FrameMemoryBudget` is the device-layer accounting carrier issued by the store.
Its limit comes from that store's `MemoryBudget` and memory source; its checked
live counter covers all outstanding charges. `FrameStore` retains its resident
statistics for pressure policy and diagnostics. Those statistics are not added a
second time to the live counter. Updating a store's configuration updates the
policy used by outstanding capabilities as well.

Auto and AutoFastest retain an existing probe frame only after validating its
layout and reserving `pixels.capacity() + original_png.capacity()`. Every live
AutoFastest candidate uses the same counter. A reservation failure stops selection
with its typed workspace error and performs the original cleanup; it does not
become an unavailable-backend attempt. Explicit selection, cache-hit construction
and Nemu paired construction do not gain a probe. Their normal capture results
are charged before the worker returns them.

The frame owns a non-clonable charge after its pixel and PNG fields. Taking the
prime and returning the command move that charge with the buffers. Dropping a
response, failed operation or locally discarded prime releases bytes when the
buffer is dropped. A retained buffer keeps its charge independently of cancellation
or a native close result. Native close authority and Unconfirmed resource retention
are unchanged. Private charge ownership prevents detaching a charge from a live
frame; checked subtraction makes an accounting underflow fail visibly.

Frame copies use fallible `try_clone`: an owned frame reserves a separate charge
from the same owner before copying. Changing or omitting an existing frame's owner
is an error. Original and copied buffers remain charged through material handling.
Entry metadata, thumbnails, PNG encoding and publication/read workspaces also
reserve from that owner. When an encoder produces a retained PNG, the retained
capacity moves from its workspace charge into the frame charge without a release
gap. Resident publication retains the original buffer until Created, verification,
Verified and any required Host pin have succeeded.

The production consumers are:

- Readonly, capture sequence and Lab observation: the observation entry creates the
  store before capture and passes it into its original pipeline afterward.
- Contained tasks: the accumulator prepares the store before its first capture;
  subsequent commands obtain the capability from the same retained pipeline.
- Resident monitor: its capture uses that probe's store; PNG preparation remains
  before CaptureCompleted, and the material pipeline follows it. Monitor publication
  uses its original frame/run/correlation and does not commit an input frame.

The kernel command, scheduled provider wrapper and device registry forward the
capability. A factory call without a memory owner cannot retain an Auto probe.
The existing standalone device tool obtains its probe capability from a pure-memory
FrameStore too. Independent synthetic frames remain uncharged until admitted into
a material owner; fixture copies are fallible and never share a physical charge.

Capacity refusal remains `frame_workspace_unavailable`; owner, accounting and
budget-source failures remain severe. Invalid incoming readonly layout retains
CaptureFailed/Indeterminate and RecognitionNotPerformed. Primary, cleanup and
backend-open evidence remain on the original request chain. Artifact, hash, I/O,
Ledger and required-pin failures retain their fatal boundaries.

This covers retention of frames already returned by the existing probes, not
pre-admission of every native allocation. It adds no native connect, capture,
input or close call, no new prime producer, cross-command prime reuse, availability
fact, bootstrap or scheduler permission. Existing request/frame/session identities
and timing samples keep their original paths.

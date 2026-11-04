# Mesa patches

Patches for GPUs that Mesa main does not run right yet. The recipe applies
the ones `_patches` in `../PKGBUILD` lists, in that order. To change the set,
edit the list, raise `pkgrel` and run `image/pack mesa-denial`.

- `turnip-no-BV-pipe-on-the-Adreno-722.patch`: the A722 has no BV pipe, so
  Turnip keeps concurrent binning off and registers no BV bin preamble on it.
  Before, the BV amble replaced the BR one, and a render pass resumed after
  preemption ran without its CCU setup for GMEM: CCU write faults at IOVA 0x0
  to 0x1240 and corrupted tiles in GPUI apps. Main at 98a55f45 still
  glitches on the Edge 70 without it (2026-11-04).

## Chasing GPU faults

A devcoredump watcher keeps every fault, not just the first:

    systemd-run --unit gpu-capture sh -c 'while :; do
      for d in /sys/class/devcoredump/devcd*; do [ -e "$d/data" ] || continue
        cat "$d/data" > /var/tmp/gpu-capture/devcd-$(date +%H%M%S).txt
        echo 1 > "$d/data"; done; sleep 0.3; done'

The msm tracepoints show who preempted whom, and when, next to each submit:

    T=/sys/kernel/tracing
    echo 1 > $T/events/drm_msm_gpu/msm_gpu_preemption_trigger/enable
    echo 1 > $T/events/drm_msm_gpu/msm_gpu_submit/enable
    echo 8192 > $T/buffer_size_kb; echo 1 > $T/tracing_on

Capture an app's submits with `FD_RD_DUMP=enable FD_RD_DUMP_PATH=DIR`. Each
submit becomes one .rd file. Don't use `FD_RD_DUMP=full`: it waits on every
submit, so preemption never happens and the bug goes away.

The freedreno tools come from the same Mesa source, built with
`-Dtools=freedreno -Dfreedreno-kmds=msm -Dgallium-drivers= -Dvulkan-drivers=
-Dplatforms= -Dglx=disabled -Degl=disabled -Dgbm=disabled -Dllvm=disabled`
and `debug/replay-overlap-and-preempt.patch` applied. Plain `replay` waits on
every submit too. With the patch, two replays on different priorities preempt
each other, the way the compositor (ring 0) preempts Turnip (ring 2):

    FD_REPLAY_PRIO=2 FD_REPLAY_REPEAT=20 replay app.rd &
    FD_REPLAY_PRIO=0 FD_REPLAY_REPEAT=20 FD_REPLAY_GAP_US=500 replay other.rd

Keep repeat counts small. A GPU stuck in hangcheck recovery blocks a software
reboot, and the phone then needs a hard reboot. `crashdec` reads the saved
dumps once their revision line names the GPU.

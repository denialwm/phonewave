/* SPDX-License-Identifier: MIT */
/* Explicit two-release transition; never accept an arbitrary kernel release. */
#ifndef PAD4_KERNEL_PAIR_H
#define PAD4_KERNEL_PAIR_H
#include <string.h>

static int pad4_release_supported(const char *release)
{
    if (!strcmp(release, RESCUE_MAINLINE_RELEASE)) return 1;
#ifdef RESCUE_MAINLINE_RELEASE_ALT
    if (!strcmp(release, RESCUE_MAINLINE_RELEASE_ALT)) return 1;
#endif
    return 0;
}

static const char *pad4_watchdog_path(const char *release)
{
    if (!strcmp(release, RESCUE_MAINLINE_RELEASE))
        return "/rescue/mainline/qcom-wdt.ko";
#ifdef RESCUE_MAINLINE_RELEASE_ALT
    if (!strcmp(release, RESCUE_MAINLINE_RELEASE_ALT))
        return "/rescue/mainline/alternate/qcom-wdt.ko";
#endif
    return NULL;
}
#endif

/* SPDX-License-Identifier: LGPL-2.1-or-later */
#ifndef GOODIX_WORKER_OPS_H
#define GOODIX_WORKER_OPS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

#define GOODIX_NATIVE_GROUP 941003u
#define GOODIX_MAX_PRINTS 10u
#define GOODIX_HAT_SIZE 69u

struct goodix_worker {
    void *context;
    int (*exchange)(void *, uint32_t *, size_t);
    int (*control)(void *, char *, size_t, int);
    int (*hat)(void *, uint8_t[GOODIX_HAT_SIZE]);
    int (*authorize)(void *, uint64_t, uint8_t[GOODIX_HAT_SIZE]);
    FILE *records;
    uint32_t ids[GOODIX_MAX_PRINTS], count, identifier, code, total;
    uint32_t attempt;
    uint8_t calibration_state;
    bool sleeping, initialized, ui_ready_required, slide_enroll, base_pending;
    /* The template group is selected and listed: verify needs no reload. */
    bool loaded;
    /* An operation's session is open: a capture follows, so an interrupt
     * leaves the sensor awake for it. */
    bool active;
    /* A verification ended without a match: its session is closed and the
     * sensor sleeps, still initialized, for the next attempt. */
    bool resting;
};

int goodix_wire_valid(const uint32_t *packet, size_t size);
int goodix_worker_prepare(struct goodix_worker *worker);
int goodix_worker_database(struct goodix_worker *worker);
int goodix_worker_operate(struct goodix_worker *worker, const char *operation, uint32_t identifier);
int goodix_worker_irq(struct goodix_worker *worker);
int goodix_worker_sleep(struct goodix_worker *worker);
#endif

/* SPDX-License-Identifier: LGPL-2.1-or-later */
#define _GNU_SOURCE
#include "goodix-worker-ops.h"
#include <errno.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

static void clear(void *data, size_t size)
{
    volatile uint8_t *p = data;
    while (size--) *p++ = 0;
}

int goodix_wire_valid(const uint32_t *p, size_t size)
{
    if (size < 32 || size % 4 || p[0] || p[3]) return 0;
    if (p[1] == 1000) {
        switch (p[2]) {
        case 0: return size == 292;
        case 1: return size == 56;
        case 2: case 9: case 14: case 15: return size == 40;
        case 3: return size == 112;
        case 4: case 8: case 13: return size == 32;
        case 6: return size == 124;
        case 7: return size == 116;
        case 10: return size == 288;
        case 11: return size == 44;
        case 12: return size == 52;
        default: return 0;
        }
    }
    if (p[1] == 1001) {
        switch (p[2]) {
        case 0: return size == 312;
        case 1: return size == 536;
        case 2: return size == 584;
        default: return 0;
        }
    }
    if (p[1] == 1002) {
        switch (p[2]) {
        case 0: return size == 40;
        case 1: case 3: return size == 48;
        case 2: return size == 288;
        case 5: case 9: return size == 36;
        case 8: case 12: return size == 32;
        case 13: return size == 68;
        default: return 0;
        }
    }
    return 0;
}

static int send_packet(struct goodix_worker *w, uint32_t *p, size_t size)
{
    if (!goodix_wire_valid(p, size)) return -EPROTO;
    int ret = w->exchange(w->context, p, size);
    if (!ret && p[1] == 1002 && p[2] == 8) w->sleeping = true;
    if (!ret && p[1] == 1002 && p[2] == 9) w->sleeping = false;
    return ret;
}

static int simple(struct goodix_worker *w, uint32_t target, uint32_t command)
{
    uint32_t packet[8] = {0, target, command};
    return send_packet(w, packet, sizeof(packet));
}

int goodix_worker_sleep(struct goodix_worker *w)
{
    return w->sleeping ? 0 : simple(w, 1002, 8);
}

static int wake(struct goodix_worker *w, uint8_t selector)
{
    if (!w->sleeping) return 0;
    uint32_t packet[9] = {0, 1002, 9};
    ((uint8_t *)packet)[0x21] = selector;
    return send_packet(w, packet, sizeof(packet));
}

static int timestamp(char *output, size_t size)
{
    struct timeval tv;
    struct tm date;
    if (gettimeofday(&tv, NULL) || !localtime_r(&tv.tv_sec, &date)) return -EIO;
    int n = snprintf(output, size, "%04d-%02d-%02d-%02d-%02d-%02d-%06ld",
                     date.tm_year + 1900, date.tm_mon + 1, date.tm_mday,
                     date.tm_hour, date.tm_min, date.tm_sec, (long)tv.tv_usec);
    return n <= 0 || (size_t)n >= size ? -EOVERFLOW : 0;
}

int goodix_worker_prepare(struct goodix_worker *w)
{
    w->sleeping = true;
    int ret = wake(w, 255);
    uint32_t init[10] = {0, 1002, 0};
    time_t now = time(NULL);
    struct tm date;
    if (now == (time_t)-1 || !localtime_r(&now, &date)) return -EIO;
    uint64_t seconds = (uint64_t)(now + date.tm_gmtoff / 3600 * 3600);
    memcpy((uint8_t *)init + 0x20, &seconds, 8);
    if (!ret) ret = send_packet(w, init, sizeof(init));
    uint32_t detect[12] = {0, 1002, 1};
    if (!ret) ret = send_packet(w, detect, sizeof(detect));
    if (!ret && !(detect[8] | detect[9] | detect[10] | detect[11])) ret = -ENODEV;
    uint32_t full[72] = {0, 1002, 2};
    memcpy((uint8_t *)full + 0xc8, "NX809J", sizeof("NX809J"));
    if (!ret) ret = send_packet(w, full, sizeof(full));
    if (!ret) {
        w->ui_ready_required = ((uint8_t *)full)[0x67] != 0;
        w->slide_enroll = ((uint8_t *)full)[0x90] != 0;
    }
    clear(full, sizeof(full));
    uint32_t algorithm[78] = {0, 1001, 0};
    algorithm[0x28 / 4] = 1;
    if (!ret) ret = timestamp((char *)algorithm + 0x35, 128);
    if (!ret) ret = send_packet(w, algorithm, sizeof(algorithm));
    if (!ret) {
        w->calibration_state = ((uint8_t *)algorithm)[0x34];
        if (!w->calibration_state) ret = -ENODATA;
    }
    clear(algorithm, sizeof(algorithm));
    if (!ret) ret = goodix_worker_sleep(w);
    uint32_t screen[11] = {0, 1000, 11};
    screen[8] = 1;
    if (!ret) ret = send_packet(w, screen, sizeof(screen));
    if (!ret) ret = wake(w, 0);
    uint32_t health[17] = {0, 1002, 13};
    if (!ret) ret = send_packet(w, health, sizeof(health));
    if (!ret && health[0x28 / 4]) ret = -EIO;
    if (!ret) ret = goodix_worker_sleep(w);
    if (!ret) ret = simple(w, 1000, 13);
    w->initialized = !ret;
    return ret;
}

static bool contains(const uint32_t *ids, uint32_t count, uint32_t id)
{
    for (uint32_t i = 0; i < count; i++) if (ids[i] == id) return true;
    return false;
}

int goodix_worker_database(struct goodix_worker *w)
{
    uint32_t group[73] = {0, 1000, 0};
    group[8] = GOODIX_NATIVE_GROUP;
    memcpy((uint8_t *)group + 0x24, "/data/vendor/goodix", sizeof("/data/vendor/goodix"));
    int ret = send_packet(w, group, sizeof(group));
    clear(group, sizeof(group));
    uint32_t list[29] = {0, 1000, 7};
    if (!ret) ret = send_packet(w, list, sizeof(list));
    if (!ret && list[8] > GOODIX_MAX_PRINTS) ret = -EPROTO;
    uint32_t ids[GOODIX_MAX_PRINTS] = {0};
    if (!ret) {
        for (uint32_t i = 0; i < list[8]; i++) {
            uint32_t gid = list[0x24 / 4 + i], id = list[0x4c / 4 + i];
            if (gid != GOODIX_NATIVE_GROUP || !id || contains(ids, i, id)) { ret = -EPROTO; break; }
            ids[i] = id;
        }
    }
    if (!ret) { w->count = list[8]; memcpy(w->ids, ids, sizeof(ids)); }
    w->loaded = !ret;
    clear(list, sizeof(list));
    return ret;
}

int goodix_worker_irq(struct goodix_worker *w)
{
    bool restore_sleep = w->sleeping && !w->active;
    int ret = wake(w, 0);
    uint32_t query[9] = {0, 1002, 5};
    if (!ret) ret = send_packet(w, query, sizeof(query));
    if (!ret && (query[8] & 2)) {
        if (restore_sleep) (void)simple(w, 1002, 12);
        ret = -EIO; /* Unexpected reset: fail the operation, never reset/retry blindly. */
    } else if (!ret && (query[8] & 1)) w->base_pending = true;
    int parked = restore_sleep ? goodix_worker_sleep(w) : 0;
    return ret ? ret : parked;
}

/* A verification that will be tried again may end at any command with
 * "rest": its session closes, but the sensor stays up for the next one. */
static int control(struct goodix_worker *w, const char *expected, bool restable)
{
    char input[16];
    int ret = w->control(w->context, input, sizeof(input), 120000);
    if (ret) return ret;
    if (restable && !strcmp(input, "rest")) w->resting = true;
    return strcmp(input, expected) ? -ECANCELED : 0;
}

static int record(struct goodix_worker *w, const char *message)
{
    return fprintf(w->records, "%s\n", message) < 0 || fflush(w->records) ? -EPIPE : 0;
}

static int capture(struct goodix_worker *w, bool enroll)
{
    int ret = wake(w, 0);
    uint32_t packet[12] = {0, 1002, 3};
    packet[8] = enroll ? 0 : w->attempt;
    packet[9] = enroll ? 1 : 2;
    ((uint8_t *)packet)[0x2e] = 1;
    for (unsigned loop = 0; !ret && loop < 11; loop++) {
        ((uint8_t *)packet)[0x2d] = loop;
        ret = send_packet(w, packet, sizeof(packet));
        if (!ret && !((uint8_t *)packet)[0x2e]) return 0;
    }
    return ret ? ret : -ETIMEDOUT;
}

static int enroll_begin(struct goodix_worker *w, bool *challenged)
{
    uint32_t challenge[10] = {0, 1000, 14};
    int ret = send_packet(w, challenge, sizeof(challenge));
    uint64_t value;
    memcpy(&value, (uint8_t *)challenge + 0x20, 8);
    clear(challenge, sizeof(challenge));
    if (ret || !value) return ret ? ret : -EPROTO;
    *challenged = true;
    uint8_t token[GOODIX_HAT_SIZE] = {0};
    if (w->authorize) ret = w->authorize(w->context, value, token);
    else {
        if (fprintf(w->records, "UFW1 challenge %llu\n", (unsigned long long)value) < 0 || fflush(w->records)) return -EPIPE;
        ret = w->hat ? w->hat(w->context, token) : -ENOKEY;
    }
    uint32_t start[28] = {0, 1000, 3};
    start[8] = GOODIX_NATIVE_GROUP;
    if (!ret) {
        memcpy((uint8_t *)start + 0x25, token, sizeof(token));
        ret = send_packet(w, start, sizeof(start));
    }
    clear(token, sizeof(token));
    clear(start, sizeof(start));
    return ret;
}

static int add_image(struct goodix_worker *w, bool first, uint32_t *remaining, uint32_t *id)
{
    uint32_t packet[134] = {0, 1001, 1};
    ((uint8_t *)packet)[0x1c0] = first && w->slide_enroll;
    ((uint8_t *)packet)[0x1c1] = 1;
    int ret = timestamp((char *)packet + 0x40, 128);
    if (!ret) ret = send_packet(w, packet, sizeof(packet));
    if (!ret) {
        uint16_t left;
        memcpy(&left, (uint8_t *)packet + 0x2c, 2);
        if (packet[8] != GOODIX_NATIVE_GROUP || left > 100) ret = -EPROTO;
        else { *remaining = left; *id = packet[9]; }
    }
    clear(packet, sizeof(packet));
    return ret;
}

/* A match also fills in the post-match packet, which stock sends after
 * the match is known; it starts no capture and decides nothing. */
static int compare(struct goodix_worker *w, uint32_t *id, uint32_t *code, uint32_t finish[13])
{
    uint32_t packet[146] = {0, 1001, 2};
    packet[8] = w->attempt;
    ((uint8_t *)packet)[0x5c] = 2;
    ((uint8_t *)packet)[0x9e] = 1;
    int ret = timestamp((char *)packet + 0xa0, 128);
    if (!ret) ret = send_packet(w, packet, sizeof(packet));
    if (ret == 1008 || ret == 1141) { *id = 0; *code = 1; ret = 0; }
    else if (ret == 1066 || ret == 1112) { *id = 0; *code = 3; ret = 0; }
    else if (!ret) {
        if (packet[0x30 / 4] != GOODIX_NATIVE_GROUP || !contains(w->ids, w->count, packet[0x34 / 4])) ret = -EPROTO;
        else {
            finish[1] = 1000; finish[2] = 12;
            finish[8] = packet[8]; finish[9] = packet[0x2c / 4];
            finish[10] = packet[0x30 / 4]; finish[11] = packet[0x34 / 4];
            *id = packet[0x34 / 4]; *code = 0;
        }
    }
    clear(packet, sizeof(packet)); /* Includes the returned authentication token. */
    return ret;
}

int goodix_worker_operate(struct goodix_worker *w, const char *operation, uint32_t identifier)
{
    w->identifier = w->code = w->total = w->attempt = 0;
    w->resting = false;
    /* Verification loads the group while armed, before the touch. */
    int ret = w->loaded && !strcmp(operation, "verify") ? 0 : goodix_worker_database(w);
    if (ret || !strcmp(operation, "list")) return ret;
    if (!strcmp(operation, "delete")) {
        if (!identifier || !contains(w->ids, w->count, identifier)) return -ENOENT;
        uint32_t old = w->count, remove[31] = {0, 1000, 6};
        remove[8] = GOODIX_NATIVE_GROUP; remove[9] = identifier;
        ret = send_packet(w, remove, sizeof(remove));
        clear(remove, sizeof(remove));
        if (!ret) ret = goodix_worker_database(w);
        if (!ret && (w->count + 1 != old || contains(w->ids, w->count, identifier))) ret = -EPROTO;
        return ret;
    }
    bool enroll = !strcmp(operation, "enroll"), check = !strcmp(operation, "check-enroll");
    bool verify = !strcmp(operation, "verify"), challenged = false, active = false;
    if (!enroll && !check && !verify) return -EINVAL;
    if ((enroll && w->count >= GOODIX_MAX_PRINTS) || (verify && !w->count)) return -ENODATA;
    uint32_t old_ids[GOODIX_MAX_PRINTS]; memcpy(old_ids, w->ids, sizeof(old_ids));
    uint32_t old_count = w->count;
    if (enroll || check) {
        ret = enroll_begin(w, &challenged);
        active = !ret;
        if (ret || check) goto cleanup;
    } else {
        uint32_t session[14] = {0, 1000, 1};
        session[8] = GOODIX_NATIVE_GROUP;
        ret = send_packet(w, session, sizeof(session));
        active = !ret;
        if (ret) goto cleanup;
    }
    w->active = active;
    ret = record(w, "UFW1 prepared");
    uint32_t last_remaining = 100;
    bool first_image = true;
    for (unsigned attempt = 0; !ret && attempt < 100; attempt++) {
        ret = control(w, "capture", verify);
        if (ret) break;
        uint32_t remaining = last_remaining, id = 0, code = 3, finish[13] = {0};
        ret = capture(w, enroll);
        /* The image is taken: the finger may lift while it is compared. */
        if (!ret) ret = record(w, "UFW1 captured");
        if (ret == 1112) ret = 0;
        else if (!ret && enroll) {
            ret = add_image(w, first_image, &remaining, &id);
            first_image = false;
            if (!ret) code = 0;
            else if (ret == 1013 || ret == 1014 || ret == 1047 || ret == 1112) ret = 0; /* Quality or duplicate area: retry. */
        } else if (!ret) ret = compare(w, &id, &code, finish);
        if (ret || remaining > last_remaining) { if (!ret) ret = -EPROTO; break; }
        if (enroll && !code && !w->total) w->total = remaining + 1;
        if (fprintf(w->records, "UFW1 sample %u %u %u %u\n", code, remaining, id, w->total) < 0 || fflush(w->records)) { ret = -EPIPE; break; }
        /* The match is out; the post-match step follows it. */
        if (verify && !code) ret = send_packet(w, finish, sizeof(finish));
        clear(finish, sizeof(finish));
        if (ret) break;
        ret = control(w, "finish", verify && code);
        if (ret) break; /* No save/result on cancellation or incomplete contact. */
        if (!enroll) { w->code = code; w->identifier = id; break; }
        if (!code && !remaining) {
            if (!id || contains(old_ids, old_count, id)) { ret = -EPROTO; break; }
            uint32_t save[10] = {0, 1000, 9};
            save[8] = GOODIX_NATIVE_GROUP; save[9] = id;
            ret = send_packet(w, save, sizeof(save));
            if (!ret) ret = goodix_worker_database(w);
            if (!ret && (w->count != old_count + 1 || !contains(w->ids, w->count, id))) ret = -EPROTO;
            for (uint32_t i = 0; !ret && i < old_count; i++) if (!contains(w->ids, w->count, old_ids[i])) ret = -EPROTO;
            if (!ret) w->identifier = id;
            break;
        }
        last_remaining = remaining;
        w->attempt++;
        ret = goodix_worker_sleep(w);
        if (!ret) ret = record(w, "UFW1 idle");
        if (!ret && attempt == 99) ret = -ETIMEDOUT;
    }
 cleanup:
    w->active = false;
    if (active) { int r = simple(w, 1000, 8); if (r) ret = r; }
    if (challenged) { int r = simple(w, 1000, 4); if (r) ret = r; }
    { int r = goodix_worker_sleep(w); if (r) ret = r; }
    return ret;
}

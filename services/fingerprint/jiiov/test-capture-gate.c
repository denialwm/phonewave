/* SPDX-License-Identifier: GPL-2.0-only */
#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
static volatile sig_atomic_t interrupted;
#include "capture-gate.h"
static void cancel(int signal_number) { (void)signal_number; interrupted=1; }

int main(void)
{
    int fds[2];
    assert(pipe(fds)==0);
    uint64_t start=monotonic_us();
    assert(capture_gate_wait(fds[0],20)==-1);
    assert(monotonic_us()-start>=19000); /* Timeout cannot grant permission. */
    assert(write(fds[1],"x",1)==1);
    assert(capture_gate_wait(fds[0],20)==-1);
    close(fds[1]);
    assert(capture_gate_wait(fds[0],20)==-1); /* Helper death/EOF. */
    close(fds[0]);

    assert(pipe(fds)==0);
    pid_t child=fork(); assert(child>=0);
    if (!child) {
        close(fds[0]);
        usleep(30000);
        assert(write(fds[1],"\1",1)==1);
        close(fds[1]); _exit(0);
    }
    close(fds[1]); start=monotonic_us();
    assert(capture_gate_wait(fds[0],500)==0);
    assert(monotonic_us()-start>=20000); /* Waited for readiness. */
    assert(capture_gate_wait(fds[0],20)==-1); /* One permission, one capture. */
    close(fds[0]); int status; assert(waitpid(child,&status,0)==child && WEXITSTATUS(status)==0);

    assert(pipe(fds)==0);
    struct sigaction action={.sa_handler=cancel};
    sigemptyset(&action.sa_mask); assert(sigaction(SIGUSR1,&action,NULL)==0);
    child=fork(); assert(child>=0);
    if (!child) { usleep(30000); kill(getppid(),SIGUSR1); _exit(0); }
    start=monotonic_us();
    assert(capture_gate_wait(fds[0],500)==-1 && interrupted);
    assert(monotonic_us()-start<300000);
    assert(write(fds[1],"\1",1)==1);
    assert(capture_gate_wait(fds[0],20)==-1); /* Cancellation beats queued ready. */
    close(fds[0]); close(fds[1]); assert(waitpid(child,&status,0)==child);
    puts("Capture gate: delayed permission, single use, EOF, invalid token, timeout and cancellation passed");
    return 0;
}

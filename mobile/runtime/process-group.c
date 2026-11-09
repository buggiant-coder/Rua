/* Own an installer process group and reap it before returning to the APK. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile sig_atomic_t group_id;
static volatile sig_atomic_t interrupted;

static void stop_group(int signal_number) {
    int saved_errno = errno;
    interrupted = signal_number;
    if (group_id > 0) kill(-(pid_t)group_id, SIGKILL);
    errno = saved_errno;
}

int main(int argc, char **argv) {
    if (argc < 2) return 125;
    sigset_t blocked, original;
    sigemptyset(&blocked);
    sigaddset(&blocked, SIGTERM);
    sigaddset(&blocked, SIGINT);
    sigaddset(&blocked, SIGHUP);
    if (sigprocmask(SIG_BLOCK, &blocked, &original) < 0) return 125;
    struct sigaction action = {0};
    action.sa_handler = stop_group;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGTERM, &action, NULL) < 0 ||
        sigaction(SIGINT, &action, NULL) < 0 ||
        sigaction(SIGHUP, &action, NULL) < 0) return 125;
    signal(SIGCHLD, SIG_DFL);
    pid_t owner = getppid();
    if (prctl(PR_SET_PDEATHSIG, SIGTERM) < 0 || getppid() != owner) return 125;
    pid_t child = fork();
    if (child < 0) return 125;
    if (child == 0) {
        if (setpgid(0, 0) < 0) _exit(125);
        signal(SIGTERM, SIG_DFL);
        signal(SIGINT, SIG_DFL);
        signal(SIGHUP, SIG_DFL);
        sigprocmask(SIG_SETMASK, &original, NULL);
        execv(argv[1], argv + 1);
        perror("installer exec");
        _exit(127);
    }
    group_id = child;
    /* Either side can win the exec race; the child sets the same group itself. */
    if (setpgid(child, child) < 0 && errno != EACCES && errno != ESRCH) {
        kill(child, SIGKILL);
    }
    sigprocmask(SIG_SETMASK, &original, NULL);
    siginfo_t info = {0};
    int result;
    do { result = waitid(P_PID, (id_t)child, &info, WEXITED | WNOWAIT); }
    while (result < 0 && errno == EINTR);
    /* Keep the leader unreaped until group cleanup: its PID cannot be reused. */
    kill(-child, SIGKILL);
    group_id = 0;
    int status = 0;
    do { result = waitpid(child, &status, 0); } while (result < 0 && errno == EINTR);
    if (interrupted) return 128 + interrupted;
    if (result < 0) return 125;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

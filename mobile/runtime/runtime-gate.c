/* Coordinate guest process admission with an exclusive environment update.
 * The shared descriptor survives exec into PRoot; its tracer owns the lease.
 * This is lifecycle coordination inside one Android UID, not a security sandbox.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <signal.h>
#include <string.h>
#include <sys/file.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 4) return 125;
    int exclusive = strcmp(argv[1], "--exclusive") == 0;
    if (!exclusive && (strcmp(argv[1], "--shared") != 0 || argc < 5)) return 125;
    int fd = open(argv[2], O_CREAT | O_RDWR | O_NOFOLLOW, 0600);
    struct stat value;
    if (fd < 0 || fstat(fd, &value) < 0 || !S_ISREG(value.st_mode)) {
        perror("runtime admission lock");
        return 125;
    }
    if (flock(fd, (exclusive ? LOCK_EX : LOCK_SH) | LOCK_NB) < 0) {
        if (errno == EWOULDBLOCK || errno == EAGAIN) {
            fputs(exclusive ? "Linux runtime is in use; close running tools, integrations and terminals before updating\n"
                            : "Linux runtime update is in progress; retry after it completes\n", stderr);
            return 75;
        }
        perror("runtime admission lock");
        return 125;
    }
    if (!exclusive) {
        if (lstat(argv[3], &value) == 0 || errno != ENOENT) {
            fputs("Linux runtime update needs recovery before new execution\n", stderr);
            return 75;
        }
        /* No CLOEXEC: PRoot retains the shared lock for its complete traced tree. */
        execv(argv[4], argv + 4);
        perror("runtime exec");
        return 127;
    }
    pid_t owner = getppid();
    if (prctl(PR_SET_PDEATHSIG, SIGKILL) < 0 || getppid() != owner) return 125;
    if (fputs("LOCKED\n", stdout) < 0 || fflush(stdout) != 0) return 125;
    char byte;
    while (read(STDIN_FILENO, &byte, 1) < 0) {
        if (errno != EINTR) return 125;
    }
    return close(fd) == 0 ? 0 : 125;
}

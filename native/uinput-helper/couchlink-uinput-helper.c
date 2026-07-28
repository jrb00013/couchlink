/* couchlink-uinput-helper — tiny privileged opener for /dev/uinput
 *
 * Installed once by the host .deb / install-host-permissions (root).
 * At runtime the host can call this with pkexec (GUI password) OR, after
 * udev+input-group is set up, never need it because /dev/uinput is already
 * group-writable.
 *
 * This binary itself is NOT setuid. Prefer udev rules. If invoked as root
 * (pkexec), it applies the permanent udev rule + reloads, then exits 0.
 *
 * Usage:
 *   couchlink-uinput-helper install-rules   # root: write udev + reload
 *   couchlink-uinput-helper check          # exit 0 if /dev/uinput is writable
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static const char *RULE_PATH = "/etc/udev/rules.d/99-couchlink-uinput.rules";
static const char *RULE_BODY =
    "# couchlink — allow users in group 'input' to create virtual DualSense pads\n"
    "KERNEL==\"uinput\", MODE=\"0660\", GROUP=\"input\", OPTIONS+=\"static_node=uinput\"\n";

static int cmd_check(void) {
  int fd = open("/dev/uinput", O_RDWR | O_NONBLOCK);
  if (fd < 0) {
    fprintf(stderr, "couchlink: cannot open /dev/uinput: %s\n", strerror(errno));
    fprintf(stderr,
            "Run the Couchlink Host installer once, or:\n"
            "  pkexec couchlink-uinput-helper install-rules\n");
    return 1;
  }
  close(fd);
  return 0;
}

static int cmd_install_rules(void) {
  if (geteuid() != 0) {
    fprintf(stderr, "couchlink: install-rules must run as root (use pkexec)\n");
    return 1;
  }

  FILE *f = fopen(RULE_PATH, "w");
  if (!f) {
    perror(RULE_PATH);
    return 1;
  }
  if (fputs(RULE_BODY, f) < 0) {
    perror("write rule");
    fclose(f);
    return 1;
  }
  fclose(f);

  /* Best-effort: load module + reload rules + trigger node */
  int st = 0;
  st |= system("modprobe uinput 2>/dev/null");
  st |= system("udevadm control --reload-rules 2>/dev/null");
  st |= system("udevadm trigger --name-match=uinput 2>/dev/null");
  (void)st;

  /* Make current session usable immediately (until reboot, udev owns mode) */
  if (chmod("/dev/uinput", 0660) != 0) {
    /* ignore — udev may recreate later */
  }
  /* Prefer chown to group input when possible */
  st = system("chgrp input /dev/uinput 2>/dev/null");
  (void)st;

  const char *user = getenv("SUDO_USER");
  if (user && *user) {
    char cmd[256];
    snprintf(cmd, sizeof cmd, "usermod -aG input '%s' 2>/dev/null", user);
    st = system(cmd);
    (void)st;
    fprintf(stdout,
            "Added %s to group 'input'. Log out and back in once "
            "(or reboot) so Couchlink Host can open /dev/uinput without prompts.\n",
            user);
  } else {
    fprintf(stdout,
            "udev rule installed. Add your user to group 'input' and re-login:\n"
            "  sudo usermod -aG input \"$USER\"\n");
  }
  return 0;
}

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: %s check|install-rules\n", argv[0]);
    return 2;
  }
  if (strcmp(argv[1], "check") == 0)
    return cmd_check();
  if (strcmp(argv[1], "install-rules") == 0)
    return cmd_install_rules();
  fprintf(stderr, "unknown command: %s\n", argv[1]);
  return 2;
}

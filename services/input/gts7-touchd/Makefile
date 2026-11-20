PREFIX ?= /usr
DESTDIR ?=
# No libc: clang cross-builds it for arm64 on any workstation.
CC = clang --target=aarch64-linux-gnu
CFLAGS = -O2 -static -nostdlib -ffreestanding -fno-builtin -fno-stack-protector -fuse-ld=lld \
	-Wl,--build-id=none -Wall -Wextra -Werror

.PHONY: all install clean
all: gts7-touchd
gts7-touchd: gts7-touchd.c
	$(CC) $(CFLAGS) -o $@ $<
install: gts7-touchd
	install -Dm755 gts7-touchd $(DESTDIR)$(PREFIX)/bin/gts7-touchd
	install -Dm644 packaging/gts7-touchd.service $(DESTDIR)$(PREFIX)/lib/systemd/system/gts7-touchd.service
	sed -i 's|/usr/bin/gts7-touchd|$(PREFIX)/bin/gts7-touchd|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/gts7-touchd.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/gts7-touchd/LICENSE
clean:
	rm -f gts7-touchd

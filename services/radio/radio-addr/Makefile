PREFIX ?= /usr
DESTDIR ?=
CC ?= cc
CFLAGS ?= -O2 -g
CFLAGS += -std=gnu11 -Wall -Wextra -Werror

.PHONY: all check install clean
all: radio-addr
radio-addr: radio-addr.c
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<
check: radio-addr
	shellcheck tests/run.sh
	sh tests/run.sh
install: radio-addr
	install -Dm755 radio-addr $(DESTDIR)$(PREFIX)/bin/radio-addr
	install -Dm644 packaging/radio-addr-bluetooth.service $(DESTDIR)$(PREFIX)/lib/systemd/system/radio-addr-bluetooth.service
	sed -i 's|/usr/bin/radio-addr|$(PREFIX)/bin/radio-addr|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/radio-addr-bluetooth.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/radio-addr/LICENSE
clean:
	rm -f radio-addr

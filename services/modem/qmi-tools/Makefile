# Build against libqrtr, e.g. make CPPFLAGS=-I$P/include LDFLAGS=-L$P/lib
CFLAGS ?= -O2 -Wall
LDLIBS = -lqrtr
TOOLS = qmi-ids qmi-nas-reject qmi-online qmi-provision qmi-status qmi-verify-pin

.PHONY: all clean
all: $(TOOLS)
clean:
	rm -f $(TOOLS)

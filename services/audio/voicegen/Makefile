# Builds the open AudioReach graph services (ARGS) for the workstation, the
# stand-in DSP and the generator. Everything lands in build/.
ARGS_URL ?= https://github.com/Audioreach/audioreach-graphservices.git
ARGS_COMMIT ?= 76d04ff
ARGS := build/args
JOBS ?= $(shell echo $$(( $$(nproc) - 2 )))

GPR := $(ARGS)/gpr
GPR_SOURCES := $(addprefix $(GPR)/core/src/, gpr_drv.c gpr_list.c gpr_main.c gpr_memq.c \
	gpr_drv_island.c gpr_list_island.c gpr_memq_island.c hash_based/gpr_session.c \
	hash_based/gpr_session_island.c) \
	$(GPR)/ext/dynamic_allocation/src/gpr_dynamic_allocation.c \
	$(GPR)/ext/logging/src/gpr_log_generic.c $(GPR)/ext/logging/stub_src/gpr_log_diag_stub.c
GPR_INCLUDES := $(addprefix -I$(GPR)/, api api/private core/inc core/inc/ar_utils/generic \
	core/src ext/logging/inc ext/dynamic_allocation/inc) \
	$(addprefix -I$(ARGS)/, ar_osal/api ar_util/api spf/api/ar_utils spf/api/apm spf/api/modules)
GSL_INCLUDES := $(addprefix -I$(ARGS)/, gsl/api ar_osal/api spf/api/apm spf/api/ar_utils \
	spf/api/modules spf/api/vcpm)

.PHONY: all clean
all: build/voicegen build/lib/libar-gpr.so.1

$(ARGS)/gsl/.libs/libar-gsl.so:
	rm -rf $(ARGS)
	git clone --quiet $(ARGS_URL) $(ARGS)
	git -C $(ARGS) checkout --quiet $(ARGS_COMMIT)
	cd $(ARGS) && autoreconf -fi && ./configure --with-dummy_diag --without-cutils --with-syslog \
		--without-ats_transport_tcp_ip
	$(MAKE) -C $(ARGS) -j$(JOBS) CFLAGS="-g -O2 -I$(CURDIR)/host -Wno-error"

# GPR with the ADSP datalink replaced by the recorder, under the soname GSL loads.
build/lib/libar-gpr.so.1: fakedsp.c $(ARGS)/gsl/.libs/libar-gsl.so
	mkdir -p build/lib
	$(CC) -shared -fPIC -O1 -g -D_GNU_SOURCE -DSESSION_ARRAY_SIZE=200 -Wno-error $(GPR_INCLUDES) \
		$(GPR_SOURCES) fakedsp.c -Wl,-soname,libar-gpr.so.1 -o $@ \
		-L$(ARGS)/ar_osal/.libs -lar-osal -lpthread

build/voicegen: voicegen.c $(ARGS)/gsl/.libs/libar-gsl.so
	$(CC) -g -Wall -o $@ voicegen.c $(GSL_INCLUDES) -L$(ARGS)/gsl/.libs -lar-gsl

clean:
	rm -rf build

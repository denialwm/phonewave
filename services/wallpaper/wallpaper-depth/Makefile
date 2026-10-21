PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo
# The Rust target of a cross build, such as aarch64-unknown-linux-gnu.
TARGET ?=
# 1 installs the weights as float16, half the size; 0 keeps them f32.
HALF ?= 1

BIN = target/$(if $(TARGET),$(TARGET)/)release/wallpaper-depth
MODELS = birefnet_lite.onnx depth_anything_v2_small.onnx
WEIGHTS = build/$(if $(filter 1,$(HALF)),f16,f32)
# A model's field from models.txt: 2 is its SHA-256, 3 its source.
pin = $(shell awk '$$1 == "$(1)" { print $$$(2) }' models.txt)

.PHONY: all check install
all: $(addprefix $(WEIGHTS)/,$(MODELS))
	$(CARGO) build --release --locked $(if $(TARGET),--target $(TARGET))
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/wallpaper-depth
	install -Dm644 -t $(DESTDIR)$(PREFIX)/share/wallpaper-depth/models $(addprefix $(WEIGHTS)/,$(MODELS))
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/wallpaper-depth/LICENSE
	install -Dm644 -t $(DESTDIR)$(PREFIX)/share/licenses/wallpaper-depth licenses/*.txt

models/%.onnx:
	@mkdir -p models
	curl -fL --retry 3 -o $@.part '$(call pin,$*.onnx,3)'
	echo '$(call pin,$*.onnx,2)  $@.part' | sha256sum -c --quiet
	mv $@.part $@

# The halving runs on the build machine, whatever TARGET is.
build/f16/%.onnx: models/%.onnx
	echo '$(call pin,$*.onnx,2)  $<' | sha256sum -c --quiet
	@mkdir -p $(@D)
	$(CARGO) run --release --locked --quiet -- half $< $@

build/f32/%.onnx: models/%.onnx
	echo '$(call pin,$*.onnx,2)  $<' | sha256sum -c --quiet
	install -Dm644 $< $@

NAME     := mac-health-monitor
LABEL    := se.codedeviate.mac-health-monitor
VERSION  := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP      := target/MacHealthMonitor.app
DEST_APP := $(HOME)/Applications/MacHealthMonitor.app
AGENT    := $(HOME)/Library/LaunchAgents/$(LABEL).plist
UID_NUM  := $(shell id -u)

.PHONY: build app icon install uninstall run

build:
	cargo build --release

app: build
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Contents/MacOS"
	cp "target/release/$(NAME)" "$(APP)/Contents/MacOS/$(NAME)"
	sed 's/@VERSION@/$(VERSION)/g' packaging/Info.plist > "$(APP)/Contents/Info.plist"
	mkdir -p "$(APP)/Contents/Resources"
	cp packaging/AppIcon.icns "$(APP)/Contents/Resources/AppIcon.icns"

# Regenerates packaging/AppIcon.icns from packaging/AppIcon.svg (needs Inkscape and ImageMagick).
icon:
	mkdir -p target/AppIcon.iconset
	inkscape packaging/AppIcon.svg --export-type=png --export-filename=target/AppIcon-1024.png -w 1024
	for s in 16 32 128 256 512; do \
		magick target/AppIcon-1024.png -resize $${s}x$${s} target/AppIcon.iconset/icon_$${s}x$${s}.png; \
		magick target/AppIcon-1024.png -resize $$((s*2))x$$((s*2)) target/AppIcon.iconset/icon_$${s}x$${s}@2x.png; \
	done
	iconutil -c icns target/AppIcon.iconset -o packaging/AppIcon.icns

# bootout is asynchronous: wait (up to ~5 s) until the job is gone before bootstrapping.
define WAIT_UNLOADED
	for i in $$(seq 25); do launchctl print gui/$(UID_NUM)/$(LABEL) >/dev/null 2>&1 || break; sleep 0.2; done
endef

install: app
	mkdir -p "$(HOME)/Applications" "$(HOME)/Library/LaunchAgents"
	rsync -a --delete "$(APP)/" "$(DEST_APP)/"
	sed 's|@EXEC@|$(DEST_APP)/Contents/MacOS/$(NAME)|' packaging/launchagent.plist > "$(AGENT)"
	launchctl bootout gui/$(UID_NUM)/$(LABEL) 2>/dev/null || true
	$(WAIT_UNLOADED)
	launchctl bootstrap gui/$(UID_NUM) "$(AGENT)"

uninstall:
	launchctl bootout gui/$(UID_NUM)/$(LABEL) 2>/dev/null || true
	$(WAIT_UNLOADED)
	rm -f "$(AGENT)"
	rm -rf "$(DEST_APP)"

run: app
	open "$(APP)"

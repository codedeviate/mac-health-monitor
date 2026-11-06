NAME     := mac-health-monitor
LABEL    := se.codedeviate.mac-health-monitor
VERSION  := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP      := target/MacHealthMonitor.app
DEST_APP := $(HOME)/Applications/MacHealthMonitor.app
AGENT    := $(HOME)/Library/LaunchAgents/$(LABEL).plist
UID_NUM  := $(shell id -u)

.PHONY: build app install uninstall run

build:
	cargo build --release

app: build
	rm -rf $(APP)
	mkdir -p $(APP)/Contents/MacOS
	cp target/release/$(NAME) $(APP)/Contents/MacOS/$(NAME)
	sed 's/@VERSION@/$(VERSION)/g' packaging/Info.plist > $(APP)/Contents/Info.plist

install: app
	mkdir -p $(HOME)/Applications $(HOME)/Library/LaunchAgents
	rsync -a --delete $(APP)/ $(DEST_APP)/
	sed 's|@EXEC@|$(DEST_APP)/Contents/MacOS/$(NAME)|' packaging/launchagent.plist > $(AGENT)
	launchctl bootout gui/$(UID_NUM)/$(LABEL) 2>/dev/null || true
	launchctl bootstrap gui/$(UID_NUM) $(AGENT)

uninstall:
	launchctl bootout gui/$(UID_NUM)/$(LABEL) 2>/dev/null || true
	rm -f $(AGENT)
	rm -rf $(DEST_APP)

run: app
	open $(APP)

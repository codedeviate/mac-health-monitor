# Out of scope and roadmap

## Out of scope (v0.1)

- **Killing or quitting processes from the menu.** v0.1 is read-only display; acting on processes is risky and is deferred.
- **Desktop notifications when something turns red.** Keeps v0.1 to the menu-bar icon alone.
- **History graphs or trends.** v0.1 shows current values only; storing history is more than this release needs.
- **CPU, thermal, battery or network monitoring.** v0.1 is limited to disk, memory and swap.
- **A preferences GUI.** Configuration is a plain text file in v0.1.
- **Code signing, notarization or distribution beyond the user's own machine.** v0.1 is built for personal use.
- **Monitoring volumes other than the boot volume.** v0.1 watches only the boot volume.

## Wishlist / roadmap

- [ ] Notifications on red
- [ ] Kill/quit actions
- [ ] History sparkline
- [ ] CPU/thermal/battery/network
- [ ] Multiple volumes
- [ ] Preferences window
- [ ] `SMAppService` login item
- [ ] Signed/notarized release
- [ ] Homebrew cask
- [ ] Docker container breakdown
- [ ] Clean-up helpers (caches, `docker system prune`)
- [ ] "Time to restart" hint (show uptime, and suggest a reboot when swap stays red)

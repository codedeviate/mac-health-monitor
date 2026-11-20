# Out of scope and roadmap

## Out of scope (v0.1)

- **Killing or quitting processes from the menu.** v0.1 is read-only display; acting on processes is risky and is deferred.
- **Desktop notifications when something turns red.** Keeps v0.1 to the menu-bar icon alone.
- **History graphs or trends.** History is now recorded and shown as a text timeline in the health report (v0.2.0); graphs are still out of scope.
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
- [ ] "Time to restart" hint (show uptime, and suggest a reboot when swap stays red): the health report now suggests a restart (v0.2.0); a menu-bar hint is still open
- [ ] "Explain with Claude": optional menu action that sends the gathered report to `claude -p` for a written analysis (needs network and tokens; must degrade gracefully when the machine is struggling)

# ax206mon

System status dashboard for AX206-protocol USB mini displays (QDtech
USB-Display, USB ID `1908:0102`). Runs as a systemd service.

## Pages

The display cycles through the pages defined in
`/etc/ax206mon/ax206mon.conf`. The clock and date are on every page.

* **Page 1 (System)** – CPU (with temperature), RAM, disk, GPU and a status footer.
  * CPU bar: dark blue = processing, light blue = I/O wait, with a thin CPU
    temperature bar under it (`temp = false` hides it).
  * RAM bar: green = apps, dark green = system/kernel, pale = buffers/cache;
    a swap bar below it (hidden if there is no swap, or with `swap = false`).
  * GPU: Radeon shows graphics load plus memory activity (VRAM fill on kernels
    without `mem_busy_percent`); Intel Arc (i915) shows the busier of the render
    and compute engines, plus compute, blitter and video rows (`ax206mon gpu` lists
    every engine's load). The GPU temperature (hwmon) is a thin bar under the
    load bar. When a page has no room for the temperature bars, the temperature is
    written next to the load percentage instead. Hidden when no supported GPU is found.
* **Page 2 (Services)** – systemd services named as `systemctl list-units -t
  service` shows them, with their state. Only as many as fit are shown; use
  `units = [...]` to choose which, or `filter` to pick running/active/failed/all.
* **Page 3 (Network)** – interfaces that have an IPv4 or IPv6 address, with
  link state. Choose interfaces with `interfaces = [...]` (none = all) and
  toggle `ipv4` / `ipv6`, and show the MAC with `mac = true`; loopback is hidden unless listed or `loopback = true`.
* **Page 4 (Compose)** – running docker compose stacks (`running/total`
  containers each). A `docker_status` widget (daemon state, container counts)
  is available too.

The service list and docker status are reused for `services_cache_seconds` /
`docker_cache_seconds` (default 10, `0` = fetch every refresh) to avoid running
`systemctl` and querying the engine on every refresh.

Default orientation is portrait (`rotate = 270`); set `rotate` in `[global]` to
0, 90, 180 or 270. The layout adapts: portrait uses stacked bars with legends,
landscape uses one-line bars and a side-by-side header.

## Configuration

See [`packaging/ax206mon.conf`](packaging/ax206mon.conf) – a `[global]` table
(rotation, refresh interval, seconds per page, ...) and one `[[page]]` table
per page listing its widgets. Pass another file with `-c`.

Preview a page without hardware: `ax206mon preview --page 2 out.png`.

## Busy schedule

When the CPU or GPU load (averaged over `average_seconds`, default 15) is above `[busy] load_threshold` (default 30 %), page 1
stays up for 12 s and then alternates with the other pages (1, 2, 1, 3, 1, 4, ...,
8 s each). Below the threshold the normal rotation applies. See
`packaging/ax206mon.conf`.

## Screen protection

The `[burnin]` table helps against LCD image retention from the static clock:
a night schedule (backlight off or dimmed between two times), a header that
shifts position/colour per page and/or over time, and an optional periodic
full-screen colour refresh. See `packaging/ax206mon.conf`.

## Notes

* Docker pages need access to the socket: `usermod -aG docker ax206mon`.
* Intel GPU load reads the i915 perf PMU, which needs `CAP_PERFMON` (granted
  by the systemd unit) or `perf_event_paranoid <= 0` when run by hand. Arc GPUs
  on the newer `xe` driver are not supported yet.

## Fonts

Text is drawn with the bundled [Noto Sans Mono](https://github.com/notofonts/notofonts.github.io)
Regular (SIL Open Font License 1.1, see `assets/fonts/`), rendered anti-aliased
at any size. `[global] font_scale` (0.6 - 1.3) scales all text; larger text means
fewer rows fit, and widgets that no longer fit above the footer are left out.

## Build

`./build-rpm.sh` builds an RPM in an AlmaLinux container; `cargo build --release`
for a plain binary.

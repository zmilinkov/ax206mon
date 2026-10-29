# ax206mon

System status dashboard for AX206-protocol USB mini displays (QDtech
USB-Display, USB ID `1908:0102`). Runs as a systemd service.

## Pages

The display cycles through the pages defined in
`/etc/ax206mon/ax206mon.conf`. The clock and date are on every page.

* **Page 1 (System)** – CPU, RAM, disk, temperature, GPU and a status footer.
  * CPU bar: dark blue = processing, light blue = I/O wait.
  * RAM bar: green = apps, dark green = system/kernel, pale = buffers/cache.
  * GPU: Radeon shows graphics load plus memory activity (VRAM fill on kernels
    without `mem_busy_percent`); Intel Arc (i915) shows the busier of the render
    and compute engines, plus compute, blitter and video rows (`ax206mon gpu` lists
    every engine's load). Hidden when no supported GPU is found.
* **Page 2 (Services)** – systemd services named as `systemctl list-units -t
  service` shows them, with their state. Only as many as fit are shown; use
  `units = [...]` to choose which, or `filter` to pick running/active/failed/all.
* **Page 3 (Network)** – interfaces that have an IPv4 or IPv6 address, with
  link state. Choose interfaces with `interfaces = [...]` (none = all) and
  toggle `ipv4` / `ipv6`, and show the MAC with `mac = true`; loopback is hidden unless listed or `loopback = true`.
* **Page 4 (Compose)** – running docker compose stacks (`running/total`
  containers each). A `docker_status` widget (daemon state, container counts)
  is available too.

Default orientation is portrait (`rotate = 270`); set `rotate` in `[global]` to
0, 90, 180 or 270. The layout adapts: portrait uses stacked bars with legends,
landscape uses one-line bars and a side-by-side header.

## Configuration

See [`packaging/ax206mon.conf`](packaging/ax206mon.conf) – a `[global]` table
(rotation, refresh interval, seconds per page, ...) and one `[[page]]` table
per page listing its widgets. Pass another file with `-c`.

Preview a page without hardware: `ax206mon preview --page 2 out.png`.

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

## Build

`./build-rpm.sh` builds an RPM in an AlmaLinux container; `cargo build --release`
for a plain binary.

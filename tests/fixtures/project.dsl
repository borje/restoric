# The sample project for UI tests and `restoric demo`: a FakeRepo history
# shaped like the mockup's (PLAN.md §3). See src/repo/fake.rs for the DSL.
host bege-laptop
root /home/bege/dev/project

snapshot 2026-07-14 09:00
  write README.md # project\n
  write go.mod module example.com/project\n
  write docs/notes.md notes\n
  write scripts/build.sh #!/bin/sh\ngo build ./...\n
  write src/main.go package main\n\nfunc main() {\n\trun()\n}\n
  write src/config.go package main\n\ntype Config struct {\n\tPort int\n}\n
  write src/util.go package main\n\nfunc clamp(v, a, b int) int {\n\treturn max(a, min(v, b))\n}\n
  write src/legacy.go package main\n\n// Deprecated: use server.go\nfunc serveOld() {}\n
  write src/api/handlers.go package api\n
  write src/api/routes.go package api\n
  write src/models/user.go package models\n
snapshot 2026-07-16 18:00
  touch src/util.go                   # metadata only
snapshot 2026-07-20 12:00
  # outside src
  append README.md more\n
snapshot 2026-07-24 12:16
  append src/main.go // flags\n
snapshot 2026-07-26 11:57
  append src/config.go // defaults\n
snapshot 2026-07-28 15:59
  append src/api/handlers.go func Health() {}\n
snapshot 2026-07-31 10:00
snapshot 2026-08-03 16:19
  write src/api/middleware.go package api\n
  append src/main.go // middleware\n
snapshot 2026-08-07 08:15
  write src/models/order.go package models\n
snapshot 2026-08-10 15:44
  append src/util.go // retries\n
snapshot 2026-08-12 09:00
  touch src/main.go                   # metadata only
snapshot 2026-08-17 16:04
  write src/feature.go package main\n
  append src/main.go // feature flag\n
snapshot 2026-08-19 21:30
snapshot 2026-08-22 11:59
  append src/config.go // timeouts\n
  append src/util.go // backoff\n
  rm src/feature.go
snapshot 2026-08-25 07:45
snapshot 2026-08-28 13:10
  # outside src
  append docs/notes.md more notes\n
snapshot 2026-09-02 20:16
  append src/main.go // signals\n
snapshot 2026-09-04 08:08
snapshot 2026-09-05 22:40
snapshot 2026-09-06 18:03
  write src/server.go package main\n\nfunc serve() {}\n
  append src/main.go // serve\n
  append src/config.go // listen address\n
  append src/api/routes.go func Routes() {}\n
  rm src/legacy.go
snapshot 2026-09-08 09:12
snapshot 2026-09-09 19:23
  append src/main.go // shutdown\n
snapshot 2026-09-11 17:05
snapshot 2026-09-13 20:51
  append src/util.go // jitter\n
  append src/models/user.go type User struct{}\n
snapshot 2026-09-16 08:30
snapshot 2026-09-18 19:55
snapshot 2026-09-20 20:35
  append src/config.go // tls\n
snapshot 2026-09-23 10:10
snapshot 2026-09-25 18:40
snapshot 2026-09-27 08:52
  append src/server.go // graceful\n
snapshot 2026-09-29 12:00
snapshot 2026-10-02 12:21

disk
  append src/main.go // unsaved edit\n

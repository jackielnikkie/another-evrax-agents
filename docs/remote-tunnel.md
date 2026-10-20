# Remote Tunnel — Mengekspos Server Remote evrax ke Internet

Panduan mengekspos fitur `/remote` (web server tiny_http di port **444**) agar bisa diakses dari luar mesin secara aman.

> **Penting**: fitur tunneling **tidak** diimplementasi di kode evrax. Akses dari luar mesin sepenuhnya ditangani oleh tool eksternal — Tailscale, Cloudflare Tunnel, atau reverse proxy / custom domain. Dokumen ini hanya menjelaskan cara menghubungkan server remote evrax ke tool tersebut.

> **Auth**: server memakai login password + **token bearer** (wajib di semua endpoint selain `/health`/`login`/`GET /`), token berlaku 24 jam, dan `/login` dibatasi (5 percobaan / 5 menit). Password default membuat server hanya melayani loopback.

## 1. Prasyarat Keamanan

Sebelum mengekspos ke internet, pastikan konfigurasi remote sudah aman.

### 1.1. Ganti password default (wajib)

Server remote evrax punya password default **`12345678`**. Selama password masih default:

- Server hanya bind ke **`127.0.0.1`** (localhost).
- Koneksi tunnel dari luar **DITOLAK**.

Untuk mengekspos ke internet, Anda **wajib** mengganti password ke nilai non-default terlebih dahulu.

### 1.2. Lokasi config

Konfigurasi remote disimpan di:

```
%APPDATA%\evrax\remote.json
```

Kunci:

| Kunci        | Arti                                                               |
| ------------ | ------------------------------------------------------------------ |
| `password`   | Password login web UI. Ganti dari `12345678` ke nilai kuat (min 8). |
| `bind`       | Alamat bind server. Default `127.0.0.1`. Bisa diubah ke `0.0.0.0`.  |
| `port`       | Port server. Default `444`.                                        |
| `public_url` | URL tunnel/domain publik (opsional, hanya untuk catatan/tampilan).  |

```json
{
  "bind": "127.0.0.1",
  "port": 444,
  "password": "GANTI_DENGAN_PASSWORD_KUAT",
  "public_url": "https://evrax.example.com"
}
```

> **Ganti password dari TUI**: buka `/remote` di evrax → pilih baris **password** → Enter → ketik password baru (min 8) → Enter. Perubahan langsung berlaku ke server yang sedang jalan **dan** ditulis ke `remote.json` ini.

### 1.3. Aturan bind

- **Default (`127.0.0.1`)** — aman. Server hanya bisa diakses dari mesin lokal; tunnel tool (Tailscale/Cloudflare/reverse proxy) yang berjalan di mesin yang sama tetap bisa meneruskan koneksi.
- **`0.0.0.0`** — mendengarkan di semua interface. **Hanya gunakan jika benar-benar perlu**, dan **jangan pernah** bind `0.0.0.0` tanpa lapisan TLS di depannya (lihat §4).

---

## 2. Tailscale Serve

Tailscale membangun jaringan private mesh antar mesin. `tailscale serve` lalu mengekspos port lokal lewat HTTPS dengan sertifikat Tailscale (TS cert) otomatis.

> **Domain**: hostname Tailscale selalu `<machine>.<tailnet>.ts.net` — **tidak bisa** memakai custom domain sendiri. Butuh custom domain → pakai Cloudflare Tunnel (§3) atau reverse proxy (§4).

### 2.1. Langkah

```bash
# Login / hubungkan mesin ke tailnet
tailscale up

# Serve port 444 di background (HTTPS otomatis via TS cert)
tailscale serve --bg 444
```

Alternatif bentuk eksplisit dengan URL:

```bash
tailscale serve --bg http://localhost:444
```

### 2.2. Akses

Setelah serve aktif, server remote bisa diakses dari perangkat lain di tailnet yang sama lewat:

```
https://<machine>.<tailnet>.ts.net
```

- `<machine>` = nama mesin di Tailscale.
- `<tailnet>` = nama tailnet Anda.

### 2.3. Konsep TS cert

- Tailscale otomatis menerbitkan sertifikat TLS untuk hostname `*.ts.net` (MagicDNS).
- Koneksi dari luar dienkripsi end-to-end, tanpa perlu membuka port di router (NAT traversal otomatis).
- Hanya perangkat dalam tailnet yang sama yang bisa mengakses — akses dikontrol oleh kebijakan ACL Tailscale, bukan oleh internet terbuka.

---

## 3. Cloudflare Tunnel

Cloudflare Tunnel (`cloudflared`) mengekspos server lokal ke internet lewat edge Cloudflare, tanpa membuka port inbound. Bisa memakai **custom domain** (domain yang dikelola Cloudflare) atau URL **random** `*.trycloudflare.com` untuk tes cepat.

### 3.0. Tes cepat (URL random, tanpa akun)

```bash
cloudflared tunnel --url http://localhost:444
```

Menghasilkan URL sementara seperti `https://random-words.trycloudflare.com`. Cocok untuk uji coba; untuk permanen pakai named tunnel di bawah.

### 3.1. Instal & login

```bash
# Pastikan cloudflared terpasang
cloudflared --version

# Login ke akun Cloudflare (buka URL yang ditampilkan untuk otorisasi)
cloudflared tunnel login
```

### 3.2. Buat tunnel

```bash
# Buat tunnel bernama "evrax" (akan menghasilkan credentials file)
cloudflared tunnel create evrax
```

### 3.3. Route DNS ke hostname

Arahkan subdomain Cloudflare ke tunnel:

```bash
cloudflared tunnel route dns evrax evrax.example.com
```

Ganti `evrax.example.com` dengan domain yang sudah dikelola Cloudflare.

### 3.4. Jalankan tunnel

```bash
cloudflared tunnel run evrax
```

### 3.5. Config file (ingress rule)

Buat config (mis. `~/.cloudflared/config.yml`) agar tunnel bisa dijalankan sebagai service dan ingress-nya jelas:

```yaml
tunnel: evrax
credentials-file: /home/<user>/.cloudflared/<tunnel-uuid>.json

ingress:
  - hostname: evrax.example.com
    service: http://localhost:444
  - service: http_status:404
```

Catatan:

- Ingress rule `service: http://localhost:444` meneruskan semua traffic hostname tersebut ke server remote evrax.
- Rule terakhir `http_status:404` menolak hostname lain (fallback).
- Jalankan dengan config: `cloudflared tunnel --config ~/.cloudflared/config.yml run`.

---

## 4. Custom Domain / Reverse Proxy

Jika punya VPS / server dengan IP publik dan domain sendiri, gunakan reverse proxy untuk terminasi TLS di depan server remote evrax.

### 4.1. Nginx

```nginx
server {
    listen 80;
    server_name evrax.example.com;

    # Redirect HTTP -> HTTPS (opsional tapi disarankan)
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl;
    server_name evrax.example.com;

    ssl_certificate     /etc/letsencrypt/live/evrax.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/evrax.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:444;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }
}
```

### 4.2. Caddy

Caddy menangani sertifikat TLS secara otomatis (via Let's Encrypt):

```caddy
evrax.example.com {
    reverse_proxy 127.0.0.1:444
}
```

### 4.3. Pentingnya TLS (HTTPS)

- Login web UI evrax **hanya memakai password** — tidak ada faktor kedua.
- Tanpa HTTPS, password dikirim dalam bentuk plaintext dan bisa disadap di jaringan.
- **Wajib** pakai HTTPS (via Caddy, Nginx + certbot, Tailscale TS cert, atau Cloudflare) sebelum mengekspos ke internet.

---

## 5. Security Checklist

Sebelum mengekspos ke internet, pastikan semua poin berikut:

- [ ] **Ganti password** dari default `12345678` ke nilai yang kuat (min 8) — lewat `/remote` di TUI atau `%APPDATA%\evrax\remote.json`.
- [ ] **Pakai HTTPS** — semua jalur (Tailscale/Cloudflare/reverse proxy) harus TLS.
- [ ] **Jangan bind `0.0.0.0` tanpa TLS** — pertahankan `bind` di `127.0.0.1` dan biarkan tunnel tool / reverse proxy yang meneruskan koneksi.
- [ ] **Batasi exposure** — Tailscale: hanya perangkat di tailnet (kontrol via ACL). Cloudflare: gunakan Cloudflare Access / policy untuk membatasi siapa yang bisa akses.
- [ ] **Aktifkan log akses** — pantau request masuk untuk mendeteksi percobaan brute-force atau akses mencurigakan.
- [ ] **Rotasi password** secara berkala, terutama jika pernah dibagikan.
- [ ] **Matikan serve/tunnel saat tidak dipakai** (`tailscale serve --bg 444` bisa di-unserve; `cloudflared tunnel` bisa dihentikan).
- [ ] **Jangan commit `remote.json`** yang berisi password ke repository / backup publik.

---

## Ringkasan Jalur

| Metode           | TLS                | Exposure                      | Catatan                                  |
| ---------------- | ------------------ | ----------------------------- | ---------------------------------------- |
| Tailscale Serve  | TS cert otomatis   | Hanya tailnet sendiri          | Tanpa buka port router                   |
| Cloudflare Tunnel| Edge Cloudflare    | Internet (via domain CF)       | Bisa tambah Cloudflare Access           |
| Reverse proxy    | Manual/Let's Encrypt | Internet (domain sendiri)    | Perlu VPS / IP publik                    |

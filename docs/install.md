# Install evrax (Windows)

## One-liner

```powershell
irm https://raw.githubusercontent.com/jackielnikkie/another-evrax-agents/main/install.ps1 | iex
```

Script mengunduh rilis terbaru (`evrax-v<versi>-x86_64-pc-windows-msvc.zip`), memverifikasi SHA-256 dari `SHA256SUMS`, lalu menaruh `evrax.exe` di `%USERPROFILE%\.evrax\bin` dan menambahkannya ke PATH user. Tidak butuh admin.

Buka terminal baru, lalu jalankan `evrax`.

## Flag

| Flag | Fungsi |
|---|---|
| `-Version <v>` | versi tertentu, mis. `-Version 0.1.0` |
| `-InstallDir <path>` | folder tujuan (default `%USERPROFILE%\.evrax\bin`) |
| `-System` | pasang ke `%ProgramFiles%\evrax\bin` + PATH machine (butuh admin) |
| `-LocalZip <path>` | pasang dari file ZIP lokal (tanpa unduh / verifikasi rilis) |
| `-Force` | timpa binary yang sudah ada |
| `-NoPath` | jangan ubah PATH |
| `-Uninstall` | hapus binary + entri PATH |
| `-Purge` | saat uninstall, hapus juga `%APPDATA%\evrax` dan `%LOCALAPPDATA%\evrax` |

Contoh:

```powershell
# versi tertentu ke folder custom
.\install.ps1 -Version 0.1.0 -InstallDir C:\tools\evrax

# pasang dari build lokal
cargo build --release -p evrax-tui
Compress-Archive target\release\evrax.exe dist\evrax.zip
.\install.ps1 -LocalZip dist\evrax.zip

# bersih total
.\install.ps1 -Uninstall -Purge
```

## Manual

1. Unduh ZIP rilis, ekstrak `evrax.exe`.
2. Taruh di folder mana pun, tambahkan foldernya ke PATH (opsional).
3. Konfigurasi `%APPDATA%\evrax\agent.json` (lihat README).

## Troubleshooting

- **`Access is denied` saat build** — ada instance `evrax` yang sedang jalan; tutup dulu (`Ctrl+C`).
- **`evrax` tidak dikenali** — buka terminal baru (PATH lama belum reload), atau pakai path lengkap.
- **SmartScreen** — installer belum ditandatangani; pilih *More info → Run anyway* setelah memverifikasi hash.
- **Antivirus memblokir** — unduh ulang dan cocokkan SHA-256 dengan `SHA256SUMS` di halaman rilis.

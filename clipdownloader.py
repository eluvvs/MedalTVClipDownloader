import os
import sys
import threading
import tkinter as tk
from tkinter import ttk
import customtkinter as ctk

CONFIG_DIR = os.path.join(os.getenv('APPDATA', os.path.expanduser('~')), 'MedalTVClipDownloader')
CONFIG_FILE = os.path.join(CONFIG_DIR, 'config.json')
HEADERS = {
    "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
}

# Deferred module placeholders to keep launch time instant
requests = None
json = None
re = None
html = None
filedialog = None
showinfo = None
showerror = None


def _load_heavy_modules():
    """Lazily load network, parsing, and dialog modules in the background or on demand."""
    global requests, json, re, html, filedialog, showinfo, showerror
    if requests is None:
        import requests as _requests
        import json as _json
        import re as _re
        import html as _html
        from tkinter import filedialog as _filedialog
        from tkinter.messagebox import showinfo as _showinfo, showerror as _showerror

        requests = _requests
        json = _json
        re = _re
        html = _html
        filedialog = _filedialog
        showinfo = _showinfo
        showerror = _showerror


class App:
    def __init__(self, root):
        self.root = root
        self.root.title("Clip Downloader :)")
        self.root.geometry("400x200")
        self.root.resizable(False, False)
        self._last_directory = None
        self.initUI()

        # Warm up heavy modules in the background while the UI is already visible
        threading.Thread(target=_load_heavy_modules, daemon=True).start()

    def get_last_directory(self):
        """Retrieve directory lazily when needed to avoid blocking startup with disk I/O."""
        if self._last_directory and os.path.isdir(self._last_directory):
            return self._last_directory
        self._last_directory = self.load_last_directory()
        return self._last_directory

    def load_last_directory(self):
        try:
            if os.path.exists(CONFIG_FILE):
                _load_heavy_modules()
                with open(CONFIG_FILE, 'r', encoding='utf-8') as f:
                    data = json.load(f)
                    saved_dir = data.get('last_directory')
                    if saved_dir and os.path.isdir(saved_dir):
                        return saved_dir
        except Exception:
            pass

        downloads_dir = os.path.join(os.path.expanduser('~'), 'Downloads')
        if os.path.isdir(downloads_dir):
            return downloads_dir
        videos_dir = os.path.join(os.path.expanduser('~'), 'Videos')
        if os.path.isdir(videos_dir):
            return videos_dir
        return os.path.expanduser('~')

    def save_last_directory(self, directory):
        if not directory or not os.path.isdir(directory):
            return
        self._last_directory = directory
        try:
            _load_heavy_modules()
            os.makedirs(CONFIG_DIR, exist_ok=True)
            with open(CONFIG_FILE, 'w', encoding='utf-8') as f:
                json.dump({'last_directory': directory}, f, indent=4)
        except Exception as e:
            print(f"Error saving config: {e}")

    def sanitize_filename(self, name):
        if not name:
            return "medal_clip"
        _load_heavy_modules()
        # Remove characters forbidden in Windows filenames
        cleaned = re.sub(r'[<>:"/\\|?*\x00-\x1f]', '', name)
        # Remove leading/trailing periods and whitespace
        cleaned = cleaned.strip('. ')
        if len(cleaned) > 150:
            cleaned = cleaned[:150].strip('. ')
        return cleaned if cleaned else "medal_clip"

    def initUI(self):
        self.root.config(bg="#302f2f")
        center_frame = tk.Frame(self.root, bg="#302f2f")
        center_frame.pack(expand=True, fill="both", padx=15, pady=10)

        self.url_label = ctk.CTkLabel(center_frame, text="Enter the URL of the clip:")
        self.url_label.grid(row=0, column=0, padx=(0, 10), pady=(5, 10), sticky="e")

        self.url_input = ctk.CTkEntry(center_frame, width=200)
        self.url_input.grid(row=0, column=1, pady=(5, 10), sticky="w")

        self.progress_bar = ttk.Progressbar(center_frame, orient="horizontal", length=320, mode="determinate")
        self.progress_bar.grid(row=1, column=0, columnspan=2, pady=10)

        self.download_button = ctk.CTkButton(center_frame, text="Download Video", command=self.start_download_thread, width=180)
        self.download_button.grid(row=2, column=0, columnspan=2, pady=5)

        self.exit_button = ctk.CTkButton(center_frame, text="Exit", command=self.exit_app, width=180)
        self.exit_button.grid(row=3, column=0, columnspan=2, pady=(5, 10))

        self.root.protocol("WM_DELETE_WINDOW", self.exit_app)

    def start_download_thread(self):
        threading.Thread(target=self.download_video, daemon=True).start()

    def fetch_clip_info(self, raw_input):
        _load_heavy_modules()
        url = raw_input.strip()
        if not url:
            return None, None

        if 'medal' not in url:
            if '/' not in url:
                url = 'https://medal.tv/clips/' + url
            else:
                return None, None

        if not url.startswith('http://') and not url.startswith('https://'):
            url = 'https://' + url

        url = url.replace('?theater=true', '').replace('&theater=true', '')

        # Extract clip ID
        clip_id_match = re.search(r'/clips/([a-zA-Z0-9_\-]+)', url)
        clip_id = clip_id_match.group(1) if clip_id_match else None

        title = None
        file_url = None

        # 1. Attempt fetching from Medal API if clip ID found
        if clip_id:
            try:
                api_url = f"https://medal.tv/api/content/{clip_id}"
                api_res = requests.get(api_url, headers={"Accept": "application/json", **HEADERS}, timeout=10)
                if api_res.status_code == 200:
                    api_data = api_res.json()
                    title = api_data.get("contentTitle") or api_data.get("videoTitle")
                    file_url = api_data.get("contentUrl")
            except Exception as e:
                print(f"Medal API fetch exception: {e}")

        # 2. Fetch HTML webpage
        try:
            res = requests.get(url, headers=HEADERS, timeout=10)
            html_text = res.text

            if not file_url and '"contentUrl":"' in html_text:
                file_url = html_text.split('"contentUrl":"')[1].split('","')[0]

            if not title:
                # Check for contentTitle / videoTitle in HTML JSON
                content_match = re.search(r'"(?:contentTitle|videoTitle)":\s*"([^"]+)"', html_text)
                if content_match:
                    title = content_match.group(1)

            if not title:
                # Check og:title meta tag
                og_match = re.search(r'<meta[^>]*property=["\']og:title["\'][^>]*content=["\'](.*?)["\']', html_text, re.IGNORECASE)
                if og_match:
                    raw_title = html.unescape(og_match.group(1))
                    clean_title = re.sub(r'\s*-\s*Clipped.*with\s*Medal\.tv.*$', '', raw_title, flags=re.IGNORECASE)
                    clean_title = re.sub(r'\s*\|\s*Medal\.tv.*$', '', clean_title, flags=re.IGNORECASE)
                    title = clean_title.strip()
        except Exception as e:
            print(f"Webpage fetch exception: {e}")

        safe_title = self.sanitize_filename(title)
        return safe_title, file_url

    def download_video(self):
        _load_heavy_modules()
        input_text = self.url_input.get().strip()
        if not input_text:
            showerror("Error", "Please enter a valid Medal clip URL or ID.")
            return

        self.download_button.configure(state="disabled")
        self.progress_bar["value"] = 0
        self.root.update()

        try:
            clip_title, file_url = self.fetch_clip_info(input_text)

            if not file_url:
                showerror("Error", "Could not find a downloadable video stream for this Medal clip. Please check the URL.")
                return

            filename = filedialog.asksaveasfilename(
                initialdir=self.get_last_directory(),
                initialfile=f"{clip_title}.mp4",
                filetypes=[("Video Files", "*.mp4"), ("All Files", "*.*")],
                defaultextension=".mp4"
            )

            if not filename:
                return

            # Save the selected directory
            self.save_last_directory(os.path.dirname(filename))

            with requests.get(file_url, stream=True, headers=HEADERS, timeout=15) as response:
                response.raise_for_status()
                total_length = response.headers.get('content-length')
                total_bytes = int(total_length) if total_length and total_length.isdigit() else None
                bytes_written = 0

                with open(filename, 'wb') as f:
                    for chunk in response.iter_content(chunk_size=1024 * 1024):  # 1 MB chunks
                        if chunk:
                            f.write(chunk)
                            bytes_written += len(chunk)
                            self.update_progress_bar(bytes_written, total_bytes)

            self.progress_bar["value"] = 100
            self.root.update()
            showinfo("Success", f"File saved successfully:\n{filename}")

        except Exception as e:
            err_type = type(e).__name__
            showerror("Download Error" if "Request" in err_type else "Error", f"An error occurred: {e}")
        finally:
            self.download_button.configure(state="normal")

    def update_progress_bar(self, bytes_written, total_bytes):
        if total_bytes and total_bytes > 0:
            done = int(100 * bytes_written / total_bytes)
            self.progress_bar["value"] = done
        else:
            self.progress_bar["value"] = 50
        self.root.update_idletasks()

    def exit_app(self):
        self.root.destroy()
        sys.exit()


if __name__ == '__main__':
    # Explicitly set dark mode to skip slow OS registry polling during CustomTkinter initialization
    ctk.set_appearance_mode("Dark")
    root = ctk.CTk()
    window = App(root)
    root.mainloop()

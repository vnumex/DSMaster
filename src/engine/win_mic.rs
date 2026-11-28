use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};

/// Lightweight Win32 WASAPI helper for reading and toggling the default Windows
/// capture (microphone) endpoint's system mute state.
pub struct WindowsMicControl {
    com_initialized: bool,
}

impl WindowsMicControl {
    pub fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            com_initialized: hr.is_ok(),
        }
    }

    fn default_mic_volume(&self) -> Option<IAudioEndpointVolume> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
            device.Activate(CLSCTX_ALL, None).ok()
        }
    }

    pub fn is_muted(&self) -> Option<bool> {
        let vol = self.default_mic_volume()?;
        unsafe { vol.GetMute().ok().map(|b| b.as_bool()) }
    }

    pub fn toggle_mute(&self) -> Option<bool> {
        let vol = self.default_mic_volume()?;
        unsafe {
            let cur = vol.GetMute().ok()?.as_bool();
            let next = !cur;
            vol.SetMute(next, std::ptr::null()).ok()?;
            Some(next)
        }
    }
}

impl Drop for WindowsMicControl {
    fn drop(&mut self) {
        if self.com_initialized {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

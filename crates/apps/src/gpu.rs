//! Запуск программ на дискретной видеокарте NVIDIA, когда экран рисует
//! встроенная (гибридный режим ноутбука). Как `prime-run`: программе — три
//! переменные окружения, остальное делает драйвер NVIDIA (PRIME render offload).

use std::path::Path;
use std::sync::OnceLock;

/// Переменные, с которыми OpenGL, EGL и Vulkan программы берут NVIDIA.
pub const OFFLOAD_ENV: [(&str, &str); 3] = [
    ("__NV_PRIME_RENDER_OFFLOAD", "1"),
    ("__VK_LAYER_NV_optimus", "NVIDIA_only"),
    ("__GLX_VENDOR_LIBRARY_NAME", "nvidia"),
];

/// Есть ли куда переносить: загружен драйвер NVIDIA и стоят его библиотеки
/// OpenGL. Проверяется один раз — видеокарта за время работы не появится.
pub fn offload_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Path::new("/proc/driver/nvidia/version").exists()
            && Path::new("/usr/share/glvnd/egl_vendor.d/10_nvidia.json").exists()
    })
}

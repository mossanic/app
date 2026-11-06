use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use jni::sys::jboolean;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};
use khronos_egl as egl;

static VULKAN_INIT: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_setSurfaceGLES(
    env: JNIEnv,
    _class: JClass,
    surface: JObject,
) -> jboolean {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("GLES: Surface created. Initializing OpenGL ES...");

    let mut init_guard = VULKAN_INIT.lock().unwrap();
    if *init_guard {
        return 1; // true
    }
    *init_guard = true;

    unsafe {
        let window = ndk::native_window::NativeWindow::from_surface(
            env.get_native_interface() as *mut _,
            surface.as_raw() as *mut _
        ).unwrap();
        
        let window_ptr = window.ptr().as_ptr() as usize;
        std::mem::forget(window);

        SHOULD_CLOSE.store(false, Ordering::SeqCst);
        thread::spawn(move || {
            run_gles(window_ptr);
        });
    }
    1 // true
}

unsafe fn run_gles(window_ptr: usize) {
    let egl = egl::DynamicInstance::<egl::EGL1_4>::load_required().expect("Failed to load EGL");
    
    // 1. Get Display
    let display = egl.get_display(egl::DEFAULT_DISPLAY).expect("Failed to get EGL display");
    let mut major = 0;
    let mut minor = 0;
    egl.initialize(display, &mut major, &mut minor).expect("Failed to initialize EGL");
    log::info!("EGL Version {}.{}", major, minor);

    // 2. Choose Config
    let config_attribs = [
        egl::RED_SIZE, 8,
        egl::GREEN_SIZE, 8,
        egl::BLUE_SIZE, 8,
        egl::ALPHA_SIZE, 8,
        egl::DEPTH_SIZE, 24,
        egl::RENDERABLE_TYPE, egl::OPENGL_ES2_BIT,
        egl::NONE,
    ];
    let config = egl.choose_first_config(display, &config_attribs).expect("Failed to choose config").unwrap();

    // 3. Create Context
    let context_attribs = [
        egl::CONTEXT_CLIENT_VERSION, 2,
        egl::NONE,
    ];
    let context = egl.create_context(display, config, None, &context_attribs).expect("Failed to create context");

    // 4. Create Surface
    let surface = egl.create_window_surface(display, config, window_ptr as egl::NativeWindowType, None).expect("Failed to create window surface");

    // 5. Make Current
    egl.make_current(display, Some(surface), Some(surface), Some(context)).expect("Failed to make current");

    // 6. Load GL Functions
    let gl = glow::Context::from_loader_function(|s| {
        egl.get_proc_address(s).map(|p| p as *const _).unwrap_or(std::ptr::null())
    });

    log::info!("Entering render loop (Green Screen).");

    // 7. Render Loop
    while !SHOULD_CLOSE.load(Ordering::SeqCst) {
        unsafe {
            gl.clear_color(0.0, 1.0, 0.0, 1.0); // Green
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egl.swap_buffers(display, surface).expect("Failed to swap buffers");
        
        // Add a tiny sleep to prevent 100% CPU spin on older devices
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    log::info!("GLES render loop exited.");
}
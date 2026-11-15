use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::ffi::CString;
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};
use ash::vk;
use ash::Entry;
use bytemuck::{Pod, Zeroable};
use mossanic_pty::Pty;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use android_raw_window::AndroidNativeWindow;

static VULKAN_INIT: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);

static VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text.vert.spv"));
static FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text.frag.spv"));

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_setSurfaceVulkan(
    env: JNIEnv, _class: JClass, surface: JObject,
) -> jni::sys::jboolean {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Trace).with_tag("MossanicRust"),
    );
    log::info!("JNI: setSurfaceVulkan called");

    let mut init_guard = VULKAN_INIT.lock().unwrap();
    if *init_guard { return 1; }
    *init_guard = true;

    unsafe {
        let window_ptr = match ndk::native_window::NativeWindow::from_surface(
            env.get_native_interface() as *mut _, surface.as_raw() as *mut _
        ) {
            Some(w) => w.ptr().as_ptr() as usize,
            None => { log::error!("Failed to create ANativeWindow"); return 0; }
        };

        SHOULD_CLOSE.store(false, Ordering::SeqCst);

        thread::spawn(move || {
            let result = std::panic::catch_unwind(|| { run_vulkan(window_ptr); });
            if result.is_err() { log::error!("VULKAN THREAD PANICKED!"); }
        });
    }
    1
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_startShell(
    _env: JNIEnv, _class: JClass,
) {
    log::info!("JNI: startShell called");
    std::thread::spawn(|| {
        let pty = match Pty::spawn("/system/bin/sh", &[], 24, 80) {
            Ok(p) => p,
            Err(e) => { log::error!("Failed to spawn PTY: {}", e); return; }
        };

        log::info!("PTY spawned successfully.");
        let _ = pty.write(b"echo Hello from mossanic-pty!\n");

        let mut buf = [0u8; 1024];
        loop {
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let output = String::from_utf8_lossy(&buf[..n]);
                    log::info!("PTY OUT: {}", output.replace('\n', "\\n").replace('\r', "\\r"));
                }
                Err(e) => { log::error!("PTY read error: {}", e); break; }
            }
        }
        log::info!("PTY loop ended.");
    });
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_clearSurface(_env: JNIEnv, _class: JClass) {
    SHOULD_CLOSE.store(true, Ordering::SeqCst);
    *VULKAN_INIT.lock().unwrap() = false;
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_resize(_env: JNIEnv, _class: JClass, _w: i32, _h: i32) {}

unsafe fn run_vulkan(window_ptr: usize) {
    log::info!("Starting Vulkan initialization...");
    let entry = Entry::load().expect("Failed to load Vulkan");

    let native_window = AndroidNativeWindow(window_ptr as *mut _);
    let window_handle = native_window.window_handle().unwrap();
    let display_handle = native_window.display_handle().unwrap();
    let raw_display_handle = display_handle.as_raw();
    let raw_window_handle = window_handle.as_raw();

    let app_name = CString::new("MossanicSpike").unwrap();
    let app_info = vk::ApplicationInfo {
        p_application_name: app_name.as_ptr(),
        api_version: vk::make_api_version(0, 1, 1, 0),
        ..Default::default()
    };

    let instance_exts = ash_window::enumerate_required_extensions(raw_display_handle).unwrap();
    let instance_create_info = vk::InstanceCreateInfo {
        p_application_info: &app_info,
        enabled_extension_count: instance_exts.len() as u32,
        pp_enabled_extension_names: instance_exts.as_ptr(),
        ..Default::default()
    };
    let instance = entry.create_instance(&instance_create_info, None).unwrap();

    let surface = ash_window::create_surface(&entry, &instance, raw_display_handle, raw_window_handle, None)
        .expect("Failed to create Vulkan surface");
    let surface_ext = ash::khr::surface::Instance::new(&entry, &instance);

    let physical_device = instance.enumerate_physical_devices().unwrap()[0];
    let device_exts = [ash::khr::swapchain::NAME.as_ptr()];
    let queue_priorities = [1.0];
    let queue_create_info = vk::DeviceQueueCreateInfo {
        queue_family_index: 0, queue_count: 1, p_queue_priorities: queue_priorities.as_ptr(), ..Default::default()
    };
    let device_create_info = vk::DeviceCreateInfo {
        queue_create_info_count: 1, p_queue_create_infos: &queue_create_info,
        enabled_extension_count: device_exts.len() as u32, pp_enabled_extension_names: device_exts.as_ptr(),
        ..Default::default()
    };
    let device = instance.create_device(physical_device, &device_create_info, None).unwrap();
    let queue = device.get_device_queue(0, 0);
    let mem_props = instance.get_physical_device_memory_properties(physical_device);

    let caps = surface_ext.get_physical_device_surface_capabilities(physical_device, surface).unwrap();
    let formats = surface_ext.get_physical_device_surface_formats(physical_device, surface).unwrap();
    let format = formats[0];

    let mut width = caps.current_extent.width;
    let mut height = caps.current_extent.height;
    if width == u32::MAX || width == 0 {
        width = native_window.get_width();
        height = native_window.get_height();
    }

    let swapchain_create_info = vk::SwapchainCreateInfoKHR {
        surface, min_image_count: caps.min_image_count + 1,
        image_format: format.format, image_color_space: format.color_space,
        image_extent: vk::Extent2D { width, height }, image_array_layers: 1,
        image_usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
        image_sharing_mode: vk::SharingMode::EXCLUSIVE,
        pre_transform: caps.current_transform, composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
        present_mode: vk::PresentModeKHR::MAILBOX, clipped: vk::TRUE, ..Default::default()
    };
    let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
    let swapchain = swapchain_loader.create_swapchain(&swapchain_create_info, None).unwrap();
    let images = swapchain_loader.get_swapchain_images(swapchain).unwrap();

    let attachments = [vk::AttachmentDescription {
        format: format.format, samples: vk::SampleCountFlags::TYPE_1,
        load_op: vk::AttachmentLoadOp::CLEAR, store_op: vk::AttachmentStoreOp::STORE,
        stencil_load_op: vk::AttachmentLoadOp::DONT_CARE, stencil_store_op: vk::AttachmentStoreOp::DONT_CARE,
        initial_layout: vk::ImageLayout::UNDEFINED, final_layout: vk::ImageLayout::PRESENT_SRC_KHR,
        ..Default::default()
    }];
    let color_refs = [vk::AttachmentReference { attachment: 0, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL }];
    let subpasses = [vk::SubpassDescription { pipeline_bind_point: vk::PipelineBindPoint::GRAPHICS, color_attachment_count: 1, p_color_attachments: color_refs.as_ptr(), ..Default::default() }];
    let dependencies = [vk::SubpassDependency { src_subpass: vk::SUBPASS_EXTERNAL, dst_subpass: 0, src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, dst_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, src_access_mask: vk::AccessFlags::empty(), dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE, dependency_flags: vk::DependencyFlags::empty(), ..Default::default() }];
    let render_pass = device.create_render_pass(&vk::RenderPassCreateInfo { attachment_count: 1, p_attachments: attachments.as_ptr(), subpass_count: 1, p_subpasses: subpasses.as_ptr(), dependency_count: 1, p_dependencies: dependencies.as_ptr(), ..Default::default() }, None).unwrap();

    let mut framebuffers = vec![];
    for image in &images {
        let view = device.create_image_view(&vk::ImageViewCreateInfo { image: *image, view_type: vk::ImageViewType::TYPE_2D, format: format.format, subresource_range: vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, level_count: 1, layer_count: 1, ..Default::default() }, ..Default::default() }, None).unwrap();
        framebuffers.push(device.create_framebuffer(&vk::FramebufferCreateInfo { render_pass, attachment_count: 1, p_attachments: &view, width, height, layers: 1, ..Default::default() }, None).unwrap());
    }

    // Create dummy pipeline & vertex buffer for the black screen (so it doesn't crash)
    let cmd_pool = device.create_command_pool(&vk::CommandPoolCreateInfo { queue_family_index: 0, ..Default::default() }, None).unwrap();
    let cmd = device.allocate_command_buffers(&vk::CommandBufferAllocateInfo { command_pool: cmd_pool, level: vk::CommandBufferLevel::PRIMARY, command_buffer_count: 1, ..Default::default() }).unwrap()[0];

    let sem_avail = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let sem_finish = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let fence = device.create_fence(&vk::FenceCreateInfo { flags: vk::FenceCreateFlags::SIGNALED, ..Default::default() }, None).unwrap();

    log::info!("Entering render loop (Pure Black).");

    while !SHOULD_CLOSE.load(Ordering::SeqCst) {
        device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
        device.reset_fences(&[fence]).unwrap();

        let acquire = swapchain_loader.acquire_next_image(swapchain, u64::MAX, sem_avail, vk::Fence::null());
        if acquire.is_err() { break; }
        let (idx, _) = acquire.unwrap();

        device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).unwrap();
        device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();

        // Pure Black Background (#000000)
        let clear = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] } }];
        device.cmd_begin_render_pass(cmd, &vk::RenderPassBeginInfo { 
            render_pass, 
            framebuffer: framebuffers[idx as usize], 
            render_area: vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } }, 
            clear_value_count: 1, 
            p_clear_values: clear.as_ptr(), 
            ..Default::default() 
        }, vk::SubpassContents::INLINE);

        device.cmd_end_render_pass(cmd);
        device.end_command_buffer(cmd).unwrap();

        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let submit = device.queue_submit(queue, &[vk::SubmitInfo { 
            wait_semaphore_count: 1, 
            p_wait_semaphores: &sem_avail, 
            p_wait_dst_stage_mask: wait_stages.as_ptr(), 
            command_buffer_count: 1, 
            p_command_buffers: &cmd, 
            signal_semaphore_count: 1, 
            p_signal_semaphores: &sem_finish, 
            ..Default::default() 
        }], fence);
        
        if submit.is_err() { break; }

        let present = swapchain_loader.queue_present(queue, &vk::PresentInfoKHR { 
            wait_semaphore_count: 1, 
            p_wait_semaphores: &sem_finish, 
            swapchain_count: 1, 
            p_swapchains: &swapchain, 
            p_image_indices: &(idx as u32), 
            ..Default::default() 
        });
        if present.is_err() { break; }
    }

    let _ = device.device_wait_idle();
}

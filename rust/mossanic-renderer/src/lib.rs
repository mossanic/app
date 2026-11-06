use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::ffi::CString;
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};
use ash::vk;
use ash::Entry;

static VULKAN_INIT: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_setSurface(
    env: JNIEnv,
    _class: JClass,
    surface: JObject,
) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("Surface created. Initializing Vulkan...");

    let mut init_guard = VULKAN_INIT.lock().unwrap();
    if *init_guard {
        return;
    }
    *init_guard = true;

    unsafe {
        let window = ndk::native_window::NativeWindow::from_surface(
            env.get_native_interface() as *mut _,
            surface.as_raw() as *mut _
        ).unwrap();
        
        let window_ptr = window.ptr().as_ptr() as usize;
        std::mem::forget(window); // Leak it for the spike

        SHOULD_CLOSE.store(false, Ordering::SeqCst);
        thread::spawn(move || {
            run_vulkan(window_ptr);
        });
    }
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_clearSurface(
    _env: JNIEnv,
    _class: JClass,
) {
    log::info!("Surface destroyed.");
    SHOULD_CLOSE.store(true, Ordering::SeqCst);
    let mut init_guard = VULKAN_INIT.lock().unwrap();
    *init_guard = false;
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_resize(
    _env: JNIEnv,
    _class: JClass,
    width: i32,
    height: i32,
) {
    log::info!("Resize: {}x{}", width, height);
}

unsafe fn run_vulkan(window_ptr: usize) {
    let entry = Entry::linked();

    // 1. Instance
    let app_name = CString::new("MossanicSpike").unwrap();
    let app_info = vk::ApplicationInfo {
        p_application_name: app_name.as_ptr(),
        api_version: vk::make_api_version(0, 1, 1, 0),
        ..Default::default()
    };

    // ash 0.38 uses static NAME constants for extensions
    let instance_exts = [
        ash::khr::surface::NAME.as_ptr(),
        ash::khr::android_surface::NAME.as_ptr(),
    ];

    let instance_create_info = vk::InstanceCreateInfo {
        p_application_info: &app_info,
        enabled_extension_count: instance_exts.len() as u32,
        pp_enabled_extension_names: instance_exts.as_ptr(),
        ..Default::default()
    };

    let instance = entry.create_instance(&instance_create_info, None).expect("Failed to create instance");

    // 2. Surface
    let android_surface = ash::khr::android_surface::Instance::new(&entry, &instance);
    let surface_create_info = vk::AndroidSurfaceCreateInfoKHR {
        window: window_ptr as *mut _,
        ..Default::default()
    };
    let surface = android_surface.create_android_surface(&surface_create_info, None).expect("Failed to create surface");

    let surface_ext = ash::khr::surface::Instance::new(&entry, &instance);

    // 3. Physical Device
    let physical_device = instance.enumerate_physical_devices().unwrap()[0];

    // 4. Present Mode (MAILBOX check)
    let present_modes = surface_ext.get_physical_device_surface_present_modes(physical_device, surface).unwrap();
    let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
        log::info!("PASS: MAILBOX supported");
        vk::PresentModeKHR::MAILBOX
    } else {
        log::info!("FALLBACK: MAILBOX unavailable, using FIFO");
        vk::PresentModeKHR::FIFO
    };

    // 5. Logical Device & Queue
    let queue_priorities = [1.0];
    let queue_create_info = vk::DeviceQueueCreateInfo {
        queue_family_index: 0,
        queue_count: 1,
        p_queue_priorities: queue_priorities.as_ptr(),
        ..Default::default()
    };

    let device_exts = [
        ash::khr::swapchain::NAME.as_ptr()
    ];

    let device_create_info = vk::DeviceCreateInfo {
        queue_create_info_count: 1,
        p_queue_create_infos: &queue_create_info,
        enabled_extension_count: device_exts.len() as u32,
        pp_enabled_extension_names: device_exts.as_ptr(),
        ..Default::default()
    };

    let device = instance.create_device(physical_device, &device_create_info, None).expect("Failed to create device");
    let queue = device.get_device_queue(0, 0);

    // 6. Swapchain Setup
    let caps = surface_ext.get_physical_device_surface_capabilities(physical_device, surface).unwrap();
    let formats = surface_ext.get_physical_device_surface_formats(physical_device, surface).unwrap();
    let format = formats[0];

    let swapchain_create_info = vk::SwapchainCreateInfoKHR {
        surface,
        min_image_count: caps.min_image_count + 1,
        image_format: format.format,
        image_color_space: format.color_space,
        image_extent: caps.current_extent,
        image_array_layers: 1,
        image_usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
        image_sharing_mode: vk::SharingMode::EXCLUSIVE,
        pre_transform: caps.current_transform,
        composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
        present_mode,
        clipped: vk::TRUE,
        ..Default::default()
    };

    let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
    let swapchain = swapchain_loader.create_swapchain(&swapchain_create_info, None).unwrap();
    let images = swapchain_loader.get_swapchain_images(swapchain).unwrap();

    // 7. Render Pass & Framebuffers
    let attachments = [vk::AttachmentDescription {
        format: format.format,
        samples: vk::SampleCountFlags::TYPE_1,
        load_op: vk::AttachmentLoadOp::CLEAR,
        store_op: vk::AttachmentStoreOp::STORE,
        stencil_load_op: vk::AttachmentLoadOp::DONT_CARE,
        stencil_store_op: vk::AttachmentStoreOp::DONT_CARE,
        initial_layout: vk::ImageLayout::UNDEFINED,
        final_layout: vk::ImageLayout::PRESENT_SRC_KHR,
        ..Default::default()
    }];

    let color_refs = [vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];
    
    let subpasses = [vk::SubpassDescription {
        pipeline_bind_point: vk::PipelineBindPoint::GRAPHICS,
        color_attachment_count: 1,
        p_color_attachments: color_refs.as_ptr(),
        ..Default::default()
    }];

    let dependencies = [vk::SubpassDependency {
        src_subpass: vk::SUBPASS_EXTERNAL,
        dst_subpass: 0,
        src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        dst_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        src_access_mask: vk::AccessFlags::empty(),
        dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        dependency_flags: vk::DependencyFlags::empty(),
        ..Default::default()
    }];

    let render_pass_create_info = vk::RenderPassCreateInfo {
        attachment_count: 1,
        p_attachments: attachments.as_ptr(),
        subpass_count: 1,
        p_subpasses: subpasses.as_ptr(),
        dependency_count: 1,
        p_dependencies: dependencies.as_ptr(),
        ..Default::default()
    };

    let render_pass = device.create_render_pass(&render_pass_create_info, None).unwrap();

    let mut framebuffers = vec![];
    for image in &images {
        let view_create_info = vk::ImageViewCreateInfo {
            image: *image,
            view_type: vk::ImageViewType::TYPE_2D,
            format: format.format,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        let view = device.create_image_view(&view_create_info, None).unwrap();

        let framebuffer_create_info = vk::FramebufferCreateInfo {
            render_pass,
            attachment_count: 1,
            p_attachments: &view,
            width: caps.current_extent.width,
            height: caps.current_extent.height,
            layers: 1,
            ..Default::default()
        };

        framebuffers.push(device.create_framebuffer(&framebuffer_create_info, None).unwrap());
    }

    // 8. Command Buffers
    let cmd_pool_create_info = vk::CommandPoolCreateInfo {
        queue_family_index: 0,
        ..Default::default()
    };
    let cmd_pool = device.create_command_pool(&cmd_pool_create_info, None).unwrap();

    let alloc_info = vk::CommandBufferAllocateInfo {
        command_pool: cmd_pool,
        level: vk::CommandBufferLevel::PRIMARY,
        command_buffer_count: 1,
        ..Default::default()
    };
    let cmd_buffers = device.allocate_command_buffers(&alloc_info).unwrap();
    let cmd = cmd_buffers[0];

    // Record initial clear command
    device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();
    let clear_values = [vk::ClearValue {
        color: vk::ClearColorValue { float32: [0.0, 0.0, 1.0, 1.0] },
    }];
    
    let render_pass_begin_info = vk::RenderPassBeginInfo {
        render_pass,
        framebuffer: framebuffers[0],
        render_area: vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: caps.current_extent,
        },
        clear_value_count: 1,
        p_clear_values: clear_values.as_ptr(),
        ..Default::default()
    };

    device.cmd_begin_render_pass(cmd, &render_pass_begin_info, vk::SubpassContents::INLINE);
    device.cmd_end_render_pass(cmd);
    device.end_command_buffer(cmd).unwrap();

    // 9. Sync Objects
    let sem_available = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let sem_finished = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    
    let fence_info = vk::FenceCreateInfo {
        flags: vk::FenceCreateFlags::SIGNALED,
        ..Default::default()
    };
    let fence = device.create_fence(&fence_info, None).unwrap();

    log::info!("Entering render loop (Blue Screen).");

    // 10. Render Loop
    while !SHOULD_CLOSE.load(Ordering::SeqCst) {
        device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
        device.reset_fences(&[fence]).unwrap();

        let (image_index, _) = swapchain_loader.acquire_next_image(swapchain, u64::MAX, sem_available, vk::Fence::null()).unwrap();

        let wait_semaphores = [sem_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [sem_finished];

        // Re-record for the specific framebuffer index
        device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).unwrap();
        device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();
        
        let render_pass_begin_info = vk::RenderPassBeginInfo {
            render_pass,
            framebuffer: framebuffers[image_index as usize],
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: caps.current_extent,
            },
            clear_value_count: 1,
            p_clear_values: clear_values.as_ptr(),
            ..Default::default()
        };

        device.cmd_begin_render_pass(cmd, &render_pass_begin_info, vk::SubpassContents::INLINE);
        device.cmd_end_render_pass(cmd);
        device.end_command_buffer(cmd).unwrap();

        let cmd_buffers = [cmd];
        let submit_info = vk::SubmitInfo {
            wait_semaphore_count: 1,
            p_wait_semaphores: wait_semaphores.as_ptr(),
            p_wait_dst_stage_mask: wait_stages.as_ptr(),
            command_buffer_count: 1,
            p_command_buffers: cmd_buffers.as_ptr(),
            signal_semaphore_count: 1,
            p_signal_semaphores: signal_semaphores.as_ptr(),
            ..Default::default()
        };

        device.queue_submit(queue, &[submit_info], fence).unwrap();

        let swapchains = [swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR {
            wait_semaphore_count: 1,
            p_wait_semaphores: signal_semaphores.as_ptr(),
            swapchain_count: 1,
            p_swapchains: swapchains.as_ptr(),
            p_image_indices: image_indices.as_ptr(),
            ..Default::default()
        };

        swapchain_loader.queue_present(queue, &present_info).unwrap();
    }

    device.device_wait_idle().unwrap();
    log::info!("Render loop exited.");
}
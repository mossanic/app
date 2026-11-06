use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::ffi::CString;
use std::ptr;
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use ash::vk;
use ash::Entry;
use ndk::native_window::NativeWindow;

static VULKAN_INIT: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_setSurface(
    mut env: JNIEnv,
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
        let window = ndk::native_window::NativeWindow::from_surface(&env, &surface).unwrap();
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
    // Swapchain recreation handled next phase
}

unsafe fn run_vulkan(window_ptr: usize) {
    let entry = Entry::linked();

    // 1. Instance
    let app_name = CString::new("MossanicSpike").unwrap();
    let app_info = vk::ApplicationInfo::builder()
        .application_name(&app_name)
        .api_version(vk::make_api_version(0, 1, 1, 0));

    let extensions = [
        ash::extensions::khr::Surface::name(),
        ash::extensions::khr::AndroidSurface::name(),
    ];

    let instance = entry.create_instance(
        &vk::InstanceCreateInfo::builder()
            .application_info(&app_info)
            .enabled_extension_names(&extensions),
        None,
    ).expect("Failed to create instance");

    // 2. Surface
    let android_surface = ash::extensions::khr::AndroidSurface::new(&entry, &instance);
    let surface = android_surface.create_android_surface(
        &vk::AndroidSurfaceCreateInfoKHR::builder().window(window_ptr as *mut _),
        None,
    ).expect("Failed to create surface");

    let surface_ext = ash::extensions::khr::Surface::new(&entry, &instance);

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
    let queue_create_info = vk::DeviceQueueCreateInfo::builder()
        .queue_family_index(0)
        .queue_priorities(&queue_priorities);
    
    let device_extensions = [ash::extensions::khr::Swapchain::name()];
    let device = instance.create_device(
        physical_device,
        &vk::DeviceCreateInfo::builder()
            .queue_create_infos(std::slice::from_ref(&queue_create_info))
            .enabled_extension_names(&device_extensions),
        None,
    ).expect("Failed to create device");

    let queue = device.get_device_queue(0, 0);

    // 6. Swapchain Setup
    let caps = surface_ext.get_physical_device_surface_capabilities(physical_device, surface).unwrap();
    let formats = surface_ext.get_physical_device_surface_formats(physical_device, surface).unwrap();
    let format = formats[0];

    let swapchain_create_info = vk::SwapchainCreateInfoKHR::builder()
        .surface(surface)
        .min_image_count(caps.min_image_count + 1)
        .image_format(format.format)
        .image_color_space(format.color_space)
        .image_extent(caps.current_extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(caps.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(present_mode)
        .clipped(true);

    let swapchain_loader = ash::extensions::khr::Swapchain::new(&instance, &device);
    let swapchain = swapchain_loader.create_swapchain(&swapchain_create_info, None).unwrap();
    let images = swapchain_loader.get_swapchain_images(swapchain).unwrap();

    // 7. Render Pass & Framebuffers
    let attachments = [vk::AttachmentDescription::builder()
        .format(format.format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)
        .build()];

    let color_refs = [vk::AttachmentReference { attachment: 0, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL }];
    let subpasses = [vk::SubpassDescription::builder().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&color_refs).build()];
    let dependencies = [vk::SubpassDependency {
        src_subpass: vk::SUBPASS_EXTERNAL, dst_subpass: 0,
        src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        dst_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        src_access_mask: vk::AccessFlags::empty(),
        dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        dependency_flags: vk::DependencyFlags::empty(),
    }];

    let render_pass = device.create_render_pass(
        &vk::RenderPassCreateInfo::builder().attachments(&attachments).subpasses(&subpasses).dependencies(&dependencies),
        None,
    ).unwrap();

    let mut framebuffers = vec![];
    for image in &images {
        let view = device.create_image_view(
            &vk::ImageViewCreateInfo::builder()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format.format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1,
                }), None,
        ).unwrap();

        framebuffers.push(device.create_framebuffer(
            &vk::FramebufferCreateInfo::builder()
                .render_pass(render_pass)
                .attachments(std::slice::from_ref(&view))
                .width(caps.current_extent.width)
                .height(caps.current_extent.height)
                .layers(1), None,
        ).unwrap());
    }

    // 8. Command Buffers
    let cmd_pool = device.create_command_pool(
        &vk::CommandPoolCreateInfo::builder().queue_family_index(0), None,
    ).unwrap();
    let cmd_buffers = device.allocate_command_buffers(
        &vk::CommandBufferAllocateInfo::builder().command_pool(cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1),
    ).unwrap();
    let cmd = cmd_buffers[0];

    // Record clear command
    device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();
    let clear_values = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 1.0, 1.0] } }];
    device.cmd_begin_render_pass(
        cmd,
        &vk::RenderPassBeginInfo::builder()
            .render_pass(render_pass).framebuffer(framebuffers[0])
            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: caps.current_extent })
            .clear_values(&clear_values),
        vk::SubpassContents::INLINE,
    );
    device.cmd_end_render_pass(cmd);
    device.end_command_buffer(cmd).unwrap();

    // 9. Sync Objects
    let sem_available = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let sem_finished = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let fence = device.create_fence(&vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED), None).unwrap();

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
        device.cmd_begin_render_pass(
            cmd,
            &vk::RenderPassBeginInfo::builder()
                .render_pass(render_pass).framebuffer(framebuffers[image_index as usize])
                .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: caps.current_extent })
                .clear_values(&clear_values),
            vk::SubpassContents::INLINE,
        );
        device.cmd_end_render_pass(cmd);
        device.end_command_buffer(cmd).unwrap();

        let cmd_buffers = [cmd];
        let submit_info = vk::SubmitInfo::builder()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&cmd_buffers)
            .signal_semaphores(&signal_semaphores);

        device.queue_submit(queue, &[submit_info.build()], fence).unwrap();

        let swapchains = [swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::builder()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);

        swapchain_loader.queue_present(queue, &present_info).unwrap();
    }

    device.device_wait_idle().unwrap();
    log::info!("Render loop exited.");
}

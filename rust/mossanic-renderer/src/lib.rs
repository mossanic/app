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

static VULKAN_INIT: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);

// Shaders compiled by the GitHub Action
static VERT_SPV: &[u8] = include_bytes!("../shaders/text.vert.spv");
static FRAG_SPV: &[u8] = include_bytes!("../shaders/text.frag.spv");

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_setSurfaceVulkan(
    mut env: JNIEnv, _class: JClass, surface: JObject,
) -> jni::sys::jboolean {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
    let mut init_guard = VULKAN_INIT.lock().unwrap();
    if *init_guard { return 1; }
    *init_guard = true;

    unsafe {
        let window = ndk::native_window::NativeWindow::from_surface(
            env.get_native_interface() as *mut _, surface.as_raw() as *mut _
        ).unwrap();
        let window_ptr = window.ptr().as_ptr() as usize;
        std::mem::forget(window);

        SHOULD_CLOSE.store(false, Ordering::SeqCst);
        thread::spawn(move || { run_vulkan(window_ptr); });
    }
    1
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_clearSurface(_env: JNIEnv, _class: JClass) {
    SHOULD_CLOSE.store(true, Ordering::SeqCst);
    *VULKAN_INIT.lock().unwrap() = false;
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_app_NativeBridge_resize(_env: JNIEnv, _class: JClass, _w: i32, _h: i32) {}

unsafe fn run_vulkan(window_ptr: usize) {
    let entry = Entry::load().expect("Failed to load Vulkan");
    let app_name = CString::new("MossanicSpike").unwrap();
    let app_info = vk::ApplicationInfo {
        p_application_name: app_name.as_ptr(),
        api_version: vk::make_api_version(0, 1, 1, 0),
        ..Default::default()
    };

    let instance_exts = [ash::khr::surface::NAME.as_ptr(), ash::khr::android_surface::NAME.as_ptr()];
    let instance_create_info = vk::InstanceCreateInfo {
        p_application_info: &app_info,
        enabled_extension_count: instance_exts.len() as u32,
        pp_enabled_extension_names: instance_exts.as_ptr(),
        ..Default::default()
    };
    let instance = entry.create_instance(&instance_create_info, None).unwrap();

    let android_surface = ash::khr::android_surface::Instance::new(&entry, &instance);
    let surface_create_info = vk::AndroidSurfaceCreateInfoKHR { window: window_ptr as *mut _, ..Default::default() };
    let surface = android_surface.create_android_surface(&surface_create_info, None).unwrap();
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

    // --- Swapchain Setup ---
    let caps = surface_ext.get_physical_device_surface_capabilities(physical_device, surface).unwrap();
    let formats = surface_ext.get_physical_device_surface_formats(physical_device, surface).unwrap();
    let format = formats[0];
    let swapchain_create_info = vk::SwapchainCreateInfoKHR {
        surface, min_image_count: caps.min_image_count + 1,
        image_format: format.format, image_color_space: format.color_space,
        image_extent: caps.current_extent, image_array_layers: 1,
        image_usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
        image_sharing_mode: vk::SharingMode::EXCLUSIVE,
        pre_transform: caps.current_transform, composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
        present_mode: vk::PresentModeKHR::MAILBOX, clipped: vk::TRUE, ..Default::default()
    };
    let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
    let swapchain = swapchain_loader.create_swapchain(&swapchain_create_info, None).unwrap();
    let images = swapchain_loader.get_swapchain_images(swapchain).unwrap();

    // --- Render Pass ---
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
        framebuffers.push(device.create_framebuffer(&vk::FramebufferCreateInfo { render_pass, attachment_count: 1, p_attachments: &view, width: caps.current_extent.width, height: caps.current_extent.height, layers: 1, ..Default::default() }, None).unwrap());
    }

    // --- Font Rasterization ---
    let font_data = include_bytes!("../assets/CascadiaCode.ttf");
    let font = swash::FontRef::from_index(font_data, 0).unwrap();
    let mut context = swash::scale::ScaleContext::new();
    let mut scaler = context.builder(font).size(64.0).build(); // Increased size to 64px for visibility
    let mut render = swash::scale::Render::new(&[]);
    let mut atlas_data = vec![0u8; 512 * 512];
    let mut vertices = Vec::new();
    let text = "hello world!";

    for (i, c) in text.chars().enumerate() {
        let glyph_id = font.charmap().map(c);
        if let Some(image) = render.render(&mut scaler, glyph_id) {
            let w = image.placement.width as usize;
            let h = image.placement.height as usize;
            
            // Layout coordinates in pixel space
            let ox = (i * 40) as i32 + 100;
            let oy = 400;
            
            // Pack into atlas
            let atlas_x = i * 64;
            let atlas_y = 0;
            
            for y in 0..h {
                for x in 0..w {
                    atlas_data[(atlas_y + y) * 512 + (atlas_x + x)] = image.data[y * w + x];
                }
            }
            
            // UV coordinates for the atlas
            let u0 = atlas_x as f32 / 512.0; let v0 = atlas_y as f32 / 512.0;
            let u1 = (atlas_x + w) as f32 / 512.0; let v1 = (atlas_y + h) as f32 / 512.0;
            
            // Pixel coordinates for the quad
            let px0 = ox as f32; let py0 = oy as f32;
            let px1 = (ox + w as i32) as f32; let py1 = (oy + h as i32) as f32;
            
            vertices.extend_from_slice(&[
                Vertex { pos: [px0, py0], uv: [u0, v0] }, Vertex { pos: [px1, py0], uv: [u1, v0] }, Vertex { pos: [px0, py1], uv: [u0, v1] },
                Vertex { pos: [px1, py0], uv: [u1, v0] }, Vertex { pos: [px1, py1], uv: [u1, v1] }, Vertex { pos: [px0, py1], uv: [u0, v1] },
            ]);
        }
    }
    
    log::info!("Generated {} vertices for text", vertices.len());
    let checksum: u32 = atlas_data.iter().map(|&x| x as u32).sum();
    log::info!("Atlas checksum: {}", checksum);

    // --- Command Pool ---
    let cmd_pool = device.create_command_pool(&vk::CommandPoolCreateInfo { queue_family_index: 0, ..Default::default() }, None).unwrap();
    let cmd = device.allocate_command_buffers(&vk::CommandBufferAllocateInfo { command_pool: cmd_pool, level: vk::CommandBufferLevel::PRIMARY, command_buffer_count: 1, ..Default::default() }).unwrap()[0];
    device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();

    // --- Texture Setup & Upload ---
    let image_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::TYPE_2D, format: vk::Format::R8_UNORM, extent: vk::Extent3D { width: 512, height: 512, depth: 1 },
        mip_levels: 1, array_layers: 1, samples: vk::SampleCountFlags::TYPE_1, tiling: vk::ImageTiling::OPTIMAL,
        usage: vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST, initial_layout: vk::ImageLayout::UNDEFINED, ..Default::default()
    };
    let texture_image = device.create_image(&image_info, None).unwrap();
    let mem_reqs = device.get_image_memory_requirements(texture_image);
    let mem_type_index = (0..mem_props.memory_type_count).find(|&i| (mem_reqs.memory_type_bits & (1 << i)) != 0 && mem_props.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)).unwrap();
    let texture_memory = device.allocate_memory(&vk::MemoryAllocateInfo { allocation_size: mem_reqs.size, memory_type_index: mem_type_index, ..Default::default() }, None).unwrap();
    device.bind_image_memory(texture_image, texture_memory, 0).unwrap();

    let staging_info = vk::BufferCreateInfo { size: atlas_data.len() as u64, usage: vk::BufferUsageFlags::TRANSFER_SRC, sharing_mode: vk::SharingMode::EXCLUSIVE, ..Default::default() };
    let staging_buffer = device.create_buffer(&staging_info, None).unwrap();
    let staging_reqs = device.get_buffer_memory_requirements(staging_buffer);
    let staging_idx = (0..mem_props.memory_type_count).find(|&i| (staging_reqs.memory_type_bits & (1 << i)) != 0 && mem_props.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)).unwrap();
    let staging_mem = device.allocate_memory(&vk::MemoryAllocateInfo { allocation_size: staging_reqs.size, memory_type_index: staging_idx, ..Default::default() }, None).unwrap();
    device.bind_buffer_memory(staging_buffer, staging_mem, 0).unwrap();
    let ptr = device.map_memory(staging_mem, 0, atlas_data.len() as u64, vk::MemoryMapFlags::empty()).unwrap();
    std::ptr::copy_nonoverlapping(atlas_data.as_ptr(), ptr as *mut u8, atlas_data.len());
    device.unmap_memory(staging_mem);

    let barrier_to_transfer = vk::ImageMemoryBarrier {
        old_layout: vk::ImageLayout::UNDEFINED, new_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED, dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image: texture_image, subresource_range: vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, level_count: 1, layer_count: 1, ..Default::default() },
        src_access_mask: vk::AccessFlags::empty(), dst_access_mask: vk::AccessFlags::TRANSFER_WRITE, ..Default::default()
    };
    device.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier_to_transfer]);

    let region = vk::BufferImageCopy {
        buffer_offset: 0, buffer_row_length: 0, buffer_image_height: 0,
        image_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        image_offset: vk::Offset3D { x: 0, y: 0, z: 0 }, image_extent: vk::Extent3D { width: 512, height: 512, depth: 1 }, ..Default::default()
    };
    device.cmd_copy_buffer_to_image(cmd, staging_buffer, texture_image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);

    let barrier_to_shader = vk::ImageMemoryBarrier {
        old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL, new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED, dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image: texture_image, subresource_range: vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, level_count: 1, layer_count: 1, ..Default::default() },
        src_access_mask: vk::AccessFlags::TRANSFER_WRITE, dst_access_mask: vk::AccessFlags::SHADER_READ, ..Default::default()
    };
    device.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[barrier_to_shader]);

    let view_info = vk::ImageViewCreateInfo { image: texture_image, view_type: vk::ImageViewType::TYPE_2D, format: vk::Format::R8_UNORM, subresource_range: vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, level_count: 1, layer_count: 1, ..Default::default() }, ..Default::default() };
    let texture_view = device.create_image_view(&view_info, None).unwrap();
    let sampler = device.create_sampler(&vk::SamplerCreateInfo { mag_filter: vk::Filter::LINEAR, min_filter: vk::Filter::LINEAR, ..Default::default() }, None).unwrap();

    // --- Vertex Buffer Setup & Upload ---
    let vbuf_info = vk::BufferCreateInfo { size: (vertices.len() * std::mem::size_of::<Vertex>()) as u64, usage: vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST, sharing_mode: vk::SharingMode::EXCLUSIVE, ..Default::default() };
    let vertex_buffer = device.create_buffer(&vbuf_info, None).unwrap();
    let vbuf_reqs = device.get_buffer_memory_requirements(vertex_buffer);
    let vbuf_idx = (0..mem_props.memory_type_count).find(|&i| (vbuf_reqs.memory_type_bits & (1 << i)) != 0 && mem_props.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)).unwrap();
    let vbuf_mem = device.allocate_memory(&vk::MemoryAllocateInfo { allocation_size: vbuf_reqs.size, memory_type_index: vbuf_idx, ..Default::default() }, None).unwrap();
    device.bind_buffer_memory(vertex_buffer, vbuf_mem, 0).unwrap();

    let v_stage_info = vk::BufferCreateInfo { size: vbuf_info.size, usage: vk::BufferUsageFlags::TRANSFER_SRC, sharing_mode: vk::SharingMode::EXCLUSIVE, ..Default::default() };
    let v_stage_buf = device.create_buffer(&v_stage_info, None).unwrap();
    let v_stage_reqs = device.get_buffer_memory_requirements(v_stage_buf);
    let v_stage_mem = device.allocate_memory(&vk::MemoryAllocateInfo { allocation_size: v_stage_reqs.size, memory_type_index: staging_idx, ..Default::default() }, None).unwrap();
    device.bind_buffer_memory(v_stage_buf, v_stage_mem, 0).unwrap();
    
    let v_ptr = device.map_memory(v_stage_mem, 0, vbuf_info.size, vk::MemoryMapFlags::empty()).unwrap();
    std::ptr::copy_nonoverlapping(vertices.as_ptr(), v_ptr as *mut Vertex, vertices.len());
    device.unmap_memory(v_stage_mem);

    device.cmd_copy_buffer(cmd, v_stage_buf, vertex_buffer, &[vk::BufferCopy { src_offset: 0, dst_offset: 0, size: vbuf_info.size }]);
    
    device.end_command_buffer(cmd).unwrap();
    
    let upload_fence = device.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
    device.queue_submit(queue, &[vk::SubmitInfo { command_buffer_count: 1, p_command_buffers: &cmd, ..Default::default() }], upload_fence).unwrap();
    device.wait_for_fences(&[upload_fence], true, u64::MAX).unwrap();
    device.destroy_fence(upload_fence, None);

    // --- Descriptor Setup ---
    let pool_info = vk::DescriptorPoolCreateInfo { max_sets: 1, pool_size_count: 1, p_pool_sizes: &vk::DescriptorPoolSize { ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 1 }, ..Default::default() };
    let descriptor_pool = device.create_descriptor_pool(&pool_info, None).unwrap();
    let layout_binding = vk::DescriptorSetLayoutBinding { binding: 0, descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 1, stage_flags: vk::ShaderStageFlags::FRAGMENT, ..Default::default() };
    let descriptor_layout = device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo { binding_count: 1, p_bindings: &layout_binding, ..Default::default() }, None).unwrap();
    let descriptor_sets = device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo { descriptor_pool, descriptor_set_count: 1, p_set_layouts: &descriptor_layout, ..Default::default() }).unwrap();
    let image_info = vk::DescriptorImageInfo { sampler, image_view: texture_view, image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL };
    device.update_descriptor_sets(&[vk::WriteDescriptorSet { dst_set: descriptor_sets[0], dst_binding: 0, descriptor_count: 1, descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, p_image_info: &image_info, ..Default::default() }], &[]);

    // --- Pipeline Creation ---
    let vert_module = device.create_shader_module(&vk::ShaderModuleCreateInfo { code_size: VERT_SPV.len(), p_code: VERT_SPV.as_ptr() as *const u32, ..Default::default() }, None).unwrap();
    let frag_module = device.create_shader_module(&vk::ShaderModuleCreateInfo { code_size: FRAG_SPV.len(), p_code: FRAG_SPV.as_ptr() as *const u32, ..Default::default() }, None).unwrap();
    let stages = [
        vk::PipelineShaderStageCreateInfo { stage: vk::ShaderStageFlags::VERTEX, module: vert_module, p_name: CString::new("main").unwrap().as_ptr(), ..Default::default() },
        vk::PipelineShaderStageCreateInfo { stage: vk::ShaderStageFlags::FRAGMENT, module: frag_module, p_name: CString::new("main").unwrap().as_ptr(), ..Default::default() },
    ];
    let binding_desc = [vk::VertexInputBindingDescription { binding: 0, stride: std::mem::size_of::<Vertex>() as u32, input_rate: vk::VertexInputRate::VERTEX }];
    let attr_desc = [
        vk::VertexInputAttributeDescription { location: 0, binding: 0, format: vk::Format::R32G32_SFLOAT, offset: 0 },
        vk::VertexInputAttributeDescription { location: 1, binding: 0, format: vk::Format::R32G32_SFLOAT, offset: 8 },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo { vertex_binding_description_count: 1, p_vertex_binding_descriptions: binding_desc.as_ptr(), vertex_attribute_description_count: 2, p_vertex_attribute_descriptions: attr_desc.as_ptr(), ..Default::default() };
    
    let pipeline_layout_info = vk::PipelineLayoutCreateInfo { set_layout_count: 1, p_set_layouts: &descriptor_layout, ..Default::default() };
    let pipeline_layout = device.create_pipeline_layout(&pipeline_layout_info, None).unwrap();

    let viewport = [vk::Viewport { x: 0.0, y: 0.0, width: caps.current_extent.width as f32, height: caps.current_extent.height as f32, min_depth: 0.0, max_depth: 1.0 }];
    let scissor = [vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: caps.current_extent }];
    let viewport_state = vk::PipelineViewportStateCreateInfo { viewport_count: 1, p_viewports: viewport.as_ptr(), scissor_count: 1, p_scissors: scissor.as_ptr(), ..Default::default() };
    
    let pipeline_info = vk::GraphicsPipelineCreateInfo {
        stage_count: 2, p_stages: stages.as_ptr(), p_vertex_input_state: &vertex_input,
        p_viewport_state: &viewport_state,
        p_input_assembly_state: &vk::PipelineInputAssemblyStateCreateInfo { topology: vk::PrimitiveTopology::TRIANGLE_LIST, ..Default::default() },
        p_rasterization_state: &vk::PipelineRasterizationStateCreateInfo { line_width: 1.0, cull_mode: vk::CullModeFlags::NONE, front_face: vk::FrontFace::COUNTER_CLOCKWISE, ..Default::default() },
        p_multisample_state: &vk::PipelineMultisampleStateCreateInfo { rasterization_samples: vk::SampleCountFlags::TYPE_1, ..Default::default() },
        p_color_blend_state: &vk::PipelineColorBlendStateCreateInfo { attachment_count: 1, p_attachments: &vk::PipelineColorBlendAttachmentState { blend_enable: vk::TRUE, src_color_blend_factor: vk::BlendFactor::SRC_ALPHA, dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA, color_write_mask: vk::ColorComponentFlags::RGBA, ..Default::default() }, ..Default::default() },
        layout: pipeline_layout, render_pass, subpass: 0, ..Default::default()
    };
    let pipeline = device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None).unwrap()[0];

    // --- Main Render Loop ---
    let sem_avail = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let sem_finish = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).unwrap();
    let fence = device.create_fence(&vk::FenceCreateInfo { flags: vk::FenceCreateFlags::SIGNALED, ..Default::default() }, None).unwrap();

    log::info!("Entering render loop (Hello World).");

    while !SHOULD_CLOSE.load(Ordering::SeqCst) {
        device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
        device.reset_fences(&[fence]).unwrap();
        let (idx, _) = swapchain_loader.acquire_next_image(swapchain, u64::MAX, sem_avail, vk::Fence::null()).unwrap();

        device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).unwrap();
        device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();
        
        // --- DEBUG: Bright Purple Clear Color ---
        let clear = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.5, 0.0, 0.5, 1.0] } }];
        device.cmd_begin_render_pass(cmd, &vk::RenderPassBeginInfo { render_pass, framebuffer: framebuffers[idx as usize], render_area: vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: caps.current_extent }, clear_value_count: 1, p_clear_values: clear.as_ptr(), ..Default::default() }, vk::SubpassContents::INLINE);
        
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
        device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
        device.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline_layout, 0, &[descriptor_sets[0]], &[]);
        device.cmd_draw(cmd, vertices.len() as u32, 1, 0, 0);
        
        device.cmd_end_render_pass(cmd);
        device.end_command_buffer(cmd).unwrap();

        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        device.queue_submit(queue, &[vk::SubmitInfo { wait_semaphore_count: 1, p_wait_semaphores: &sem_avail, p_wait_dst_stage_mask: wait_stages.as_ptr(), command_buffer_count: 1, p_command_buffers: &cmd, signal_semaphore_count: 1, p_signal_semaphores: &sem_finish, ..Default::default() }], fence).unwrap();
        
        swapchain_loader.queue_present(queue, &vk::PresentInfoKHR { wait_semaphore_count: 1, p_wait_semaphores: &sem_finish, swapchain_count: 1, p_swapchains: &swapchain, p_image_indices: &(idx as u32), ..Default::default() }).unwrap();
    }

    device.device_wait_idle().unwrap();
}

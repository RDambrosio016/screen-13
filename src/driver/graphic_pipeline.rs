use ash::vk::PushConstantRange;

use {
    super::{
        DescriptorBindingMap, DescriptorSetLayout, Device, DriverError, PipelineDescriptorInfo,
        SampleCount, Shader,
    },
    crate::{as_u32_slice, ptr::Shared},
    archery::SharedPointerKind,
    ash::vk,
    derive_builder::Builder,
    log::trace,
    ordered_float::OrderedFloat,
    std::{collections::BTreeMap, ffi::CString, thread::panicking},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DepthStencilMode {
    pub back: StencilMode,
    pub bounds_test: bool,
    pub compare_op: vk::CompareOp,
    pub depth_test: bool,
    pub depth_write: bool,
    pub front: StencilMode,
    pub min: OrderedFloat<f32>,
    pub max: OrderedFloat<f32>,
    pub stencil_test: bool,
}

impl DepthStencilMode {
    pub(super) fn into_vk(self) -> vk::PipelineDepthStencilStateCreateInfo {
        vk::PipelineDepthStencilStateCreateInfo {
            back: self.back.into_vk(),
            depth_bounds_test_enable: self.bounds_test as _,
            depth_compare_op: self.compare_op,
            depth_test_enable: self.depth_test as _,
            depth_write_enable: self.depth_write as _,
            front: self.front.into_vk(),
            max_depth_bounds: *self.max,
            min_depth_bounds: *self.min,
            stencil_test_enable: self.stencil_test as _,
            ..Default::default()
        }
    }
}

impl Default for DepthStencilMode {
    fn default() -> Self {
        Self {
            back: StencilMode::Noop,
            bounds_test: false,
            compare_op: vk::CompareOp::GREATER_OR_EQUAL,
            depth_test: true,
            depth_write: true,
            front: StencilMode::Noop,
            min: OrderedFloat(0.0),
            max: OrderedFloat(1.0),
            stencil_test: false,
        }
    }
}

#[derive(Debug)]
pub struct GraphicPipeline<P>
where
    P: SharedPointerKind,
{
    pub descriptor_bindings: DescriptorBindingMap,
    pub descriptor_info: PipelineDescriptorInfo<P>,
    device: Shared<Device<P>, P>,
    pub info: GraphicPipelineInfo,
    pub layout: vk::PipelineLayout,
    pub push_constant_ranges: Vec<PushConstantRange>,
    shader_modules: Vec<vk::ShaderModule>,
    pub state: GraphicPipelineState,
}

impl<P> GraphicPipeline<P>
where
    P: SharedPointerKind,
{
    pub fn create<S>(
        device: &Shared<Device<P>, P>,
        info: impl Into<GraphicPipelineInfo>,
        shaders: impl IntoIterator<Item = S>,
    ) -> Result<Self, DriverError>
    where
        S: Into<Shader>,
    {
        trace!("create");

        let device = Shared::clone(device);
        let info = info.into();
        let shaders = shaders
            .into_iter()
            .map(|shader| shader.into())
            .collect::<Vec<Shader>>();
        let descriptor_bindings = Shader::merge_descriptor_bindings(
            shaders
                .iter()
                .map(|shader| shader.descriptor_bindings(&device))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let stages = shaders
            .iter()
            .map(|shader| shader.stage)
            .reduce(|j, k| j | k)
            .unwrap_or_default();
        let descriptor_info =
            PipelineDescriptorInfo::create(&device, &descriptor_bindings, stages)?;
        let descriptor_sets_layouts = descriptor_info
            .layouts
            .iter()
            .map(|(_, descriptor_set_layout)| **descriptor_set_layout)
            .collect::<Box<[_]>>();

        let push_constant_ranges = shaders
            .iter()
            .map(|shader| shader.push_constant_ranges())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        unsafe {
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::builder()
                        .set_layouts(&descriptor_sets_layouts)
                        .push_constant_ranges(&push_constant_ranges),
                    None,
                )
                .map_err(|_| DriverError::Unsupported)?;
            let shader_info = shaders
                .iter()
                .map(|shader| {
                    let shader_module_create_info = vk::ShaderModuleCreateInfo {
                        code_size: shader.spirv.len(),
                        p_code: shader.spirv.as_ptr() as *const u32,
                        ..Default::default()
                    };
                    let shader_module = device
                        .create_shader_module(&shader_module_create_info, None)
                        .map_err(|_| DriverError::Unsupported)?;
                    let shader_stage = Stage {
                        flags: shader.stage,
                        module: shader_module,
                        name: CString::new(shader.entry_name.as_str()).unwrap(),
                    };

                    Result::<_, DriverError>::Ok((shader_module, shader_stage))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut shader_modules = vec![];
            let mut stages = vec![];
            shader_info
                .into_iter()
                .for_each(|(shader_module, shader_stage)| {
                    shader_modules.push(shader_module);
                    stages.push(shader_stage);
                });

            let vertex_input_state = VertexInputState {
                vertex_attribute_descriptions: vec![],
                vertex_binding_descriptions: vec![],
            };
            let input_assembly_state = vk::PipelineInputAssemblyStateCreateInfo {
                topology: vk::PrimitiveTopology::TRIANGLE_LIST,
                ..Default::default()
            };
            let rasterization_state = vk::PipelineRasterizationStateCreateInfo {
                front_face: vk::FrontFace::COUNTER_CLOCKWISE,
                line_width: 1.0,
                polygon_mode: vk::PolygonMode::FILL,
                cull_mode: if info.two_sided {
                    ash::vk::CullModeFlags::NONE
                } else {
                    ash::vk::CullModeFlags::BACK
                },
                ..Default::default()
            };
            let multisample_state = MultisampleState {
                rasterization_samples: info.samples,
                ..Default::default()
            };

            Ok(Self {
                descriptor_bindings,
                descriptor_info,
                device,
                info,
                layout,
                push_constant_ranges,
                shader_modules,
                state: GraphicPipelineState {
                    input_assembly_state,
                    layout,
                    multisample_state,
                    rasterization_state,
                    stages,
                    vertex_input_state,
                },
            })
        }
    }
}

impl<P> Drop for GraphicPipeline<P>
where
    P: SharedPointerKind,
{
    fn drop(&mut self) {
        if panicking() {
            return;
        }

        unsafe {
            self.device.destroy_pipeline_layout(self.layout, None);
        }

        for shader_module in self.shader_modules.drain(..) {
            unsafe {
                self.device.destroy_shader_module(shader_module, None);
            }
        }
    }
}

#[derive(Builder, Clone, Debug, Default, PartialEq)]
#[builder(pattern = "owned")]
pub struct GraphicPipelineInfo {
    #[builder(default)]
    pub depth_stencil: Option<DepthStencilMode>,
    #[builder(default = "SampleCount::X1")]
    pub samples: SampleCount,
    #[builder(default)]
    pub two_sided: bool,
}

impl GraphicPipelineInfo {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> GraphicPipelineInfoBuilder {
        GraphicPipelineInfoBuilder::default()
    }
}

impl From<GraphicPipelineInfoBuilder> for GraphicPipelineInfo {
    fn from(info: GraphicPipelineInfoBuilder) -> Self {
        info.build().unwrap()
    }
}

#[derive(Debug)]
pub struct GraphicPipelineState {
    pub input_assembly_state: vk::PipelineInputAssemblyStateCreateInfo,
    pub layout: vk::PipelineLayout,
    pub multisample_state: MultisampleState,
    pub rasterization_state: vk::PipelineRasterizationStateCreateInfo,
    pub stages: Vec<Stage>,
    pub vertex_input_state: VertexInputState,
}

#[derive(Debug, Default)]
pub struct MultisampleState {
    pub alpha_to_coverage_enable: bool,
    pub alpha_to_one_enable: bool,
    pub flags: vk::PipelineMultisampleStateCreateFlags,
    pub min_sample_shading: f32,
    pub rasterization_samples: SampleCount,
    pub sample_mask: Vec<u32>,
    pub sample_shading_enable: bool,
}

#[derive(Debug)]
pub struct Stage {
    pub flags: vk::ShaderStageFlags,
    pub module: vk::ShaderModule,
    pub name: CString,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StencilMode {
    Noop, // TODO: Provide some sensible modes
}

impl StencilMode {
    fn into_vk(self) -> vk::StencilOpState {
        match self {
            Self::Noop => vk::StencilOpState {
                fail_op: vk::StencilOp::KEEP,
                pass_op: vk::StencilOp::KEEP,
                depth_fail_op: vk::StencilOp::KEEP,
                compare_op: vk::CompareOp::ALWAYS,
                ..Default::default()
            },
        }
    }
}

impl Default for StencilMode {
    fn default() -> Self {
        Self::Noop
    }
}

#[derive(Debug)]
pub struct VertexInputState {
    pub vertex_binding_descriptions: Vec<vk::VertexInputBindingDescription>,
    pub vertex_attribute_descriptions: Vec<vk::VertexInputAttributeDescription>,
}

use {
    super::{
        AccelerationStructureBinding, AccelerationStructureLeaseBinding,
        AccelerationStructureLeaseNode, AccelerationStructureNode, BufferBinding,
        BufferLeaseBinding, BufferLeaseNode, BufferNode, ImageBinding, ImageLeaseBinding,
        ImageLeaseNode, ImageNode, PassRef, PipelinePassRef, RenderGraph, Resolver,
        SwapchainImageBinding, SwapchainImageNode,
    },
    crate::{
        driver::{
            AccelerationStructure, Buffer, ComputePipeline, GraphicPipeline, Image,
            RayTracePipeline, SwapchainImage,
        },
        Lease,
    },
    archery::{SharedPointer, SharedPointerKind},
};

/// A marker trait that says some graph object can transition into a different
/// graph object; it is a one-way transition unless the other direction has
/// been implemented too.
pub trait Edge<Graph> {
    type Result;
}

macro_rules! graph_edge {
    ($src:ident -> $dst:ident) => {
        impl<P> Edge<RenderGraph<P>> for $src<P>
        where
            P: SharedPointerKind,
        {
            type Result = $dst<P>;
        }
    };
}

// Edges that can be bound as nodes to the render graph:
// Ex: RenderGraph::bind_node(&mut self, binding: X) -> Y
graph_edge!(AccelerationStructure -> AccelerationStructureNode);
graph_edge!(AccelerationStructureBinding -> AccelerationStructureNode);
graph_edge!(AccelerationStructureLeaseBinding -> AccelerationStructureLeaseNode);
graph_edge!(Buffer -> BufferNode);
graph_edge!(BufferBinding -> BufferNode);
graph_edge!(BufferLeaseBinding -> BufferLeaseNode);
graph_edge!(Image -> ImageNode);
graph_edge!(ImageBinding -> ImageNode);
graph_edge!(ImageLeaseBinding -> ImageLeaseNode);
graph_edge!(SwapchainImage -> SwapchainImageNode);
graph_edge!(SwapchainImageBinding -> SwapchainImageNode);

// Edges that can be unbound from the render graph:
// Ex: RenderGraph::unbind_node(&mut self, node: X) -> Y
graph_edge!(AccelerationStructureNode -> AccelerationStructureBinding);
graph_edge!(BufferNode -> BufferBinding);
graph_edge!(BufferLeaseNode -> BufferLeaseBinding);
graph_edge!(ImageNode -> ImageBinding);
graph_edge!(ImageLeaseNode -> ImageLeaseBinding);
graph_edge!(SwapchainImageNode -> SwapchainImageBinding);

macro_rules! graph_lease_edge {
    ($src:ident -> $dst:ident) => {
        impl<P> Edge<RenderGraph<P>> for Lease<$src<P>, P>
        where
            P: SharedPointerKind,
        {
            type Result = $dst<P>;
        }
    };
}

graph_lease_edge!(AccelerationStructure -> AccelerationStructureNode);
graph_lease_edge!(BufferBinding -> BufferLeaseNode);
graph_lease_edge!(ImageBinding -> ImageLeaseNode);

// Specialized edges for pipelines added to a pass:
// Ex: PassRef::bind_pipeline(&mut self, pipeline: X) -> PipelinePassRef
macro_rules! pipeline_edge {
    ($name:ident) => {
        paste::paste! {
            impl<'a, P> Edge<PassRef<'a, P>> for &'a SharedPointer<[<$name Pipeline>]<P>, P>
            where
                P: SharedPointerKind + Send,
            {
                type Result = PipelinePassRef<'a, [<$name Pipeline>]<P>, P>;
            }
        }
    };
}

pipeline_edge!(Compute);
pipeline_edge!(Graphic);
pipeline_edge!(RayTrace);

macro_rules! resolver_edge {
    ($src:ident -> $dst:ident) => {
        impl<P> Edge<Resolver<P>> for $src<P>
        where
            P: SharedPointerKind + Send,
        {
            type Result = $dst<P>;
        }
    };
}

// Edges that can be unbound from a resolved render graph:
// (You get the full real actual swapchain image woo hoo!)
resolver_edge!(SwapchainImageNode -> SwapchainImage);

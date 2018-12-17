struct ComputeInput {
    uint3 idx : SV_DispatchThreadID;
};


struct PushData {
    uint batch_size;
    float delta;
};



struct Instance {
    float2 position;
    float2 size;
    float4 color;
};

[[vk::binding(0, 1)]]
RWStructuredBuffer<Instance> instances;

[[vk::binding(1, 1)]]
RWStructuredBuffer<float2> velocities;


[[vk::push_constant]]
ConstantBuffer<PushData> push_data;

void ComputeMain(ComputeInput input)
{
    uint idx = input.idx.x + (push_data.batch_size * input.idx.y);

    uint len;
    instances.GetDimensions(len);

    if (idx > len)
        return;

    float2 position = instances[idx].position += velocities[idx] * push_data.delta;

    if (position.x < -1.0 || position.x > 1.0) {
        position.x = clamp(position.x, -1.0, 1.0);
        velocities[idx].x *= -1.0;
    }

    if (position.y < -1.0 || position.y > 1.0) {
        position.y = clamp(position.y, -1.0, 1.0);
        velocities[idx].y *= -1.0;
    }

    instances[idx].position = position;
}
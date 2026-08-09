// iterative test_line shared by the trace and gather kernels
// kernel and the gather kernel the including file must define
// trace_tnode_binding before including this see trace_bspcomp for the
// semantics notes (epsilon rules, linecontent, the coplanar watermark)
//
// the traversal stack is sized to real bsp depth and can never
// grow past the deepest root to leaf path) instead of 128, halving the
// per thread state that limits how many threads the gpu keeps resident
// entries stay point encoded: a fraction encoded variant (12 bytes/entry)
// was tried and rejected - reconstructing points as orig + f * dir drifts
// from the cpu's nested midpoint lerps by ulps, which flips borderline
// traces and turns into full light on/off deltas on isolated samples the
// host refuses maps whose tree is deeper than trace_stack_size (gpucpp
// keeps the constant in sync)

const int CONTENTS_EMPTY = -1;
const int CONTENTS_SOLID = -2;
const int CONTENTS_SKY = -6;

const int PLANE_X = 0;
const int PLANE_Y = 1;
const int PLANE_Z = 2;

const float ON_EPSILON = 0.04;
const float NORMAL_EPSILON = 0.00001;
const int TRACE_STACK_SIZE = 64;

const int TRACE_FLAG_COPLANAR = 1;
const int TRACE_FLAG_STACK_OVERFLOW = 2;

struct Tnode
{
    float normalX;
    float normalY;
    float normalZ;
    float dist;
    int type;
    int child0;
    int child1;
    int pad;
};

layout(std430, set = 0, binding = TRACE_TNODE_BINDING) readonly buffer TnodeBuffer
{
    Tnode tnodes[];
};

// walks the segment through the tnodes; returns the contents classification
// and, on contents_sky, the entry point into the sky leaf
int traceLine(float segSX, float segSY, float segSZ,
              float segEX, float segEY, float segEZ,
              out float skyX, out float skyY, out float skyZ,
              inout int traceFlags)
{
    int linecontent = 0;
    bool pendingSky = false;
    bool skyWritten = false;
    skyX = 0.0;
    skyY = 0.0;
    skyZ = 0.0;
    int watermark = -1;

    int stackNode[TRACE_STACK_SIZE];
    float stackSX[TRACE_STACK_SIZE];
    float stackSY[TRACE_STACK_SIZE];
    float stackSZ[TRACE_STACK_SIZE];
    float stackEX[TRACE_STACK_SIZE];
    float stackEY[TRACE_STACK_SIZE];
    float stackEZ[TRACE_STACK_SIZE];
    int top = 0;

    int node = 0;
    precise float curStartX = segSX;
    precise float curStartY = segSY;
    precise float curStartZ = segSZ;
    precise float curStopX = segEX;
    precise float curStopY = segEY;
    precise float curStopZ = segEZ;

    int result = CONTENTS_EMPTY;
    bool done = false;

    while (!done)
    {
        bool aborted = false;
        while (node >= 0)
        {
            Tnode tn = tnodes[node];
            precise float front;
            precise float back;
            if (tn.type == PLANE_X)
            {
                front = curStartX - tn.dist;
                back = curStopX - tn.dist;
            }
            else if (tn.type == PLANE_Y)
            {
                front = curStartY - tn.dist;
                back = curStopY - tn.dist;
            }
            else if (tn.type == PLANE_Z)
            {
                front = curStartZ - tn.dist;
                back = curStopZ - tn.dist;
            }
            else
            {
                front = (curStartX * tn.normalX + curStartY * tn.normalY + curStartZ * tn.normalZ) - tn.dist;
                back = (curStopX * tn.normalX + curStopY * tn.normalY + curStopZ * tn.normalZ) - tn.dist;
            }

            if (front > ON_EPSILON / 2 && back > ON_EPSILON / 2)
            {
                node = tn.child0;
                continue;
            }
            if (front < -ON_EPSILON / 2 && back < -ON_EPSILON / 2)
            {
                node = tn.child1;
                continue;
            }
            if (abs(front) <= ON_EPSILON && abs(back) <= ON_EPSILON)
            {
                traceFlags |= TRACE_FLAG_COPLANAR;
                if (top >= TRACE_STACK_SIZE)
                {
                    traceFlags |= TRACE_FLAG_STACK_OVERFLOW;
                    result = CONTENTS_SOLID;
                    aborted = true;
                    break;
                }
                if (watermark < 0)
                    watermark = top;
                stackNode[top] = tn.child1;
                stackSX[top] = curStartX;
                stackSY[top] = curStartY;
                stackSZ[top] = curStartZ;
                stackEX[top] = curStopX;
                stackEY[top] = curStopY;
                stackEZ[top] = curStopZ;
                top++;
                node = tn.child0;
                continue;
            }
            {
                bool side = (front - back) < 0.0;
                precise float frac = front / (front - back);
                frac = clamp(frac, 0.0, 1.0);
                precise float midX = curStartX + (curStopX - curStartX) * frac;
                precise float midY = curStartY + (curStopY - curStartY) * frac;
                precise float midZ = curStartZ + (curStopZ - curStartZ) * frac;
                if (top >= TRACE_STACK_SIZE)
                {
                    traceFlags |= TRACE_FLAG_STACK_OVERFLOW;
                    result = CONTENTS_SOLID;
                    aborted = true;
                    break;
                }
                int nearChild = side ? tn.child1 : tn.child0;
                int farChild = side ? tn.child0 : tn.child1;
                stackNode[top] = farChild;
                stackSX[top] = midX;
                stackSY[top] = midY;
                stackSZ[top] = midZ;
                stackEX[top] = curStopX;
                stackEY[top] = curStopY;
                stackEZ[top] = curStopZ;
                top++;
                node = nearChild;
                curStopX = midX;
                curStopY = midY;
                curStopZ = midZ;
            }
        }
        if (aborted)
            break;

        if (node != linecontent)
        {
            if (node == CONTENTS_SOLID)
            {
                result = CONTENTS_SOLID;
                break;
            }
            else if (node == CONTENTS_SKY)
            {
                if (!skyWritten)
                {
                    skyX = curStartX;
                    skyY = curStartY;
                    skyZ = curStartZ;
                    skyWritten = true;
                }
                if (watermark >= 0)
                {
                    pendingSky = true;
                    if (top > watermark + 1)
                        top = watermark + 1;
                }
                else
                {
                    result = CONTENTS_SKY;
                    break;
                }
            }
            else if (linecontent != 0)
            {
                result = CONTENTS_SOLID;
                break;
            }
            else
            {
                linecontent = node;
            }
        }

        if (top == 0)
        {
            result = pendingSky ? CONTENTS_SKY : CONTENTS_EMPTY;
            break;
        }
        if (watermark >= 0 && top <= watermark)
        {
            if (pendingSky)
            {
                result = CONTENTS_SKY;
                break;
            }
            watermark = -1;
        }
        top--;
        node = stackNode[top];
        curStartX = stackSX[top];
        curStartY = stackSY[top];
        curStartZ = stackSZ[top];
        curStopX = stackEX[top];
        curStopY = stackEY[top];
        curStopZ = stackEZ[top];
    }

    return result;
}

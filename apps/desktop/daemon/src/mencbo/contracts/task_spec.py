"""统一任务规范 TaskSpec（契约 2）。

每个子项目（memory、infra-core 等）通过自身 ``tasks.py`` 暴露符合本规范的
清单，由 mencbo 守护器的统一调度中心（Phase 2）在运行时动态加载。
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

#: 允许的调度类型。
VALID_SCHEDULE_TYPES = frozenset({"interval", "cron"})

#: interval 类型允许的时间字段。
_INTERVAL_KEYS = frozenset({"days", "hours", "minutes", "seconds"})

#: cron 类型允许的日历字段。
_CRON_KEYS = frozenset({"minute", "hour", "day", "month", "weekday", "year"})


def _validate_schedule_value(schedule_type: str, value: dict) -> None:
    if not isinstance(value, dict) or not value:
        raise ValueError(f"schedule_value 必须为非空 dict，收到: {value!r}")
    allowed = _INTERVAL_KEYS if schedule_type == "interval" else _CRON_KEYS
    unknown = set(value) - allowed
    if unknown:
        raise ValueError(
            f"schedule_type={schedule_type!r} 不支持的字段 {sorted(unknown)}，"
            f"允许字段: {sorted(allowed)}"
        )
    for key, raw in value.items():
        if isinstance(raw, bool) or not isinstance(raw, int) or raw < 0:
            raise ValueError(f"schedule_value[{key!r}] 必须为非负整数，收到: {raw!r}")


@dataclass(slots=True)
class TaskSpec:
    """统一任务规范（契约 2）。

    Attributes:
        task_id: 全局唯一标识，采用 ``namespace:name`` 形式，如
            ``"memory:dreaming"``、``"infra:heartbeat"``。
        display_name: 客户端界面展示名称（中文）。
        schedule_type: ``"interval"``（间隔触发）或 ``"cron"``（定点触发）。
        schedule_value: 调度参数，如 ``{"hours": 4}`` 或
            ``{"hour": 19, "minute": 0}``。
        handler: 实际执行的无参可调用对象。
        timeout_sec: 单次执行超时限制（秒）。
        enabled: 是否参与调度。
    """

    task_id: str
    display_name: str
    schedule_type: str
    schedule_value: dict
    handler: Callable[[], object]
    timeout_sec: int = 300
    enabled: bool = True

    def __post_init__(self) -> None:
        if not isinstance(self.task_id, str) or not self.task_id.strip():
            raise ValueError(f"task_id 必须为非空字符串，收到: {self.task_id!r}")
        if any(ch.isspace() for ch in self.task_id):
            raise ValueError(f"task_id 不允许包含空白字符: {self.task_id!r}")
        if not isinstance(self.display_name, str) or not self.display_name.strip():
            raise ValueError(f"display_name 必须为非空字符串，收到: {self.display_name!r}")
        if self.schedule_type not in VALID_SCHEDULE_TYPES:
            raise ValueError(
                f"schedule_type 必须为 {sorted(VALID_SCHEDULE_TYPES)} 之一，"
                f"收到: {self.schedule_type!r}"
            )
        _validate_schedule_value(self.schedule_type, self.schedule_value)
        if not callable(self.handler):
            raise TypeError(f"handler 必须可调用，收到: {type(self.handler).__name__}")
        if (
            isinstance(self.timeout_sec, bool)
            or not isinstance(self.timeout_sec, int)
            or self.timeout_sec <= 0
        ):
            raise ValueError(f"timeout_sec 必须为正整数，收到: {self.timeout_sec!r}")

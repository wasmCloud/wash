package condition

import (
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// Managed Condition types.
const (
	// TypeReady resources are believed to be ready to handle work.
	TypeReady ConditionType = "Ready"

	// TypeSynced resources are believed to be in sync with the
	// Kubernetes resources that manage their lifecycle.
	TypeSynced ConditionType = "Synced"
)

// Reasons a resource is or is not ready.
const (
	ReasonAvailable   ConditionReason = "Available"
	ReasonUnavailable ConditionReason = "Unavailable"
	ReasonCreating    ConditionReason = "Creating"
	ReasonDeleting    ConditionReason = "Deleting"
)

// Reasons a resource is or is not synced.
const (
	ReasonReconcilePending ConditionReason = "ReconcilePending"
	ReasonReconcileSuccess ConditionReason = "ReconcileSuccess"
	ReasonReconcileError   ConditionReason = "ReconcileError"
)

// Creating returns a condition that indicates the resource is currently
// being created.
func Creating() Condition {
	return Condition{
		Type:               TypeReady,
		Status:             ConditionFalse,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonCreating,
	}
}

// Deleting returns a condition that indicates the resource is currently
// being deleted.
func Deleting() Condition {
	return Condition{
		Type:               TypeReady,
		Status:             ConditionFalse,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonDeleting,
	}
}

// Available returns a condition that indicates the resource is
// currently observed to be available for use.
func Available() Condition {
	return Condition{
		Type:               TypeReady,
		Status:             ConditionTrue,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonAvailable,
	}
}

// Unavailable returns a condition that indicates the resource is not
// currently available for use. Unavailable should be set only when Crossplane
// expects the resource to be available but knows it is not, for example
// because its API reports it is unhealthy.
func Unavailable() Condition {
	return Condition{
		Type:               TypeReady,
		Status:             ConditionFalse,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonUnavailable,
	}
}

// ReconcilePending returns a condition indicating the beginning of a reconciliation.
func ReconcilePending() Condition {
	return Condition{
		Type:               TypeSynced,
		Status:             ConditionFalse,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonReconcilePending,
	}
}

// ReconcileSuccess returns a condition indicating that Crossplane successfully
// completed the most recent reconciliation of the resource.
func ReconcileSuccess() Condition {
	return Condition{
		Type:               TypeSynced,
		Status:             ConditionTrue,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonReconcileSuccess,
	}
}

// ReconcileError returns a condition indicating that Crossplane encountered an
// error while reconciling the resource. This could mean Crossplane was
// unable to update the resource to reflect its desired state, or that
// Crossplane was unable to determine the current actual state of the resource.
func ReconcileError(err error) Condition {
	return Condition{
		Type:               TypeSynced,
		Status:             ConditionFalse,
		LastTransitionTime: metav1.Now(),
		Reason:             ReasonReconcileError,
		Message:            err.Error(),
	}
}

// ReadyCondition generate ready condition for conditionType
func ReadyCondition(condType ConditionType) Condition {
	return Condition{
		Type:               condType,
		Status:             ConditionTrue,
		Reason:             ReasonAvailable,
		LastTransitionTime: metav1.NewTime(time.Now()),
	}
}

// ErrorCondition generate error condition for conditionType and error
func ErrorCondition(condType ConditionType, reason ConditionReason, err error) Condition {
	return Condition{
		Type:               condType,
		Status:             ConditionFalse,
		LastTransitionTime: metav1.NewTime(time.Now()),
		Reason:             reason,
		Message:            err.Error(),
	}
}

// UnknownCondition generate error condition for conditionType and error
func UnknownCondition(condType ConditionType, reason ConditionReason, message string) Condition {
	return Condition{
		Type:               condType,
		Status:             ConditionUnknown,
		LastTransitionTime: metav1.NewTime(time.Now()),
		Reason:             reason,
		Message:            message,
	}
}

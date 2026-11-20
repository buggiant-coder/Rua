import type { DeviceDto } from "@/api/modules/sync";
import type {
  ProjectCreationOutcome,
  ProjectDescriptor,
} from "@/api/types/project";
import ProjectEditDialog from "./ProjectEditDialog";

export default function CreateProjectDialog({
  open,
  ...props
}: {
  open: boolean;
  devices: DeviceDto[];
  onClose: () => void;
  onCreated: (outcome: ProjectCreationOutcome) => void;
  onViewDuplicate: (project: ProjectDescriptor) => void;
}) {
  return open ? <ProjectEditDialog {...props} /> : null;
}

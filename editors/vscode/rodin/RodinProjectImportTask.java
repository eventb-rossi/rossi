/*
 * Reviewable source for the Rodin project importer used by the VS Code
 * "Open in Rodin" command.
 *
 * The extension cannot rely on a JDK being present on the user's machine, so it
 * ships this class *precompiled* and embedded as base64 in
 * `editors/vscode/src/rossiCommands.ts`
 * (`RODIN_PROJECT_IMPORT_TASK_CLASS_BASE64`). At runtime the extension writes
 * the decoded class to a temporary directory and invokes it as an Ant task
 * through Rodin's bundled headless Eclipse Ant runner
 * (`org.eclipse.ant.core.antRunner`), which registers the exported `.project`
 * into a fresh Rodin workspace before Rodin is launched.
 *
 * This file is the human-readable source the embedded class was built from; it
 * is kept here purely for review and reproducibility — it is NOT compiled by
 * any build step in this repo. `RodinProjectImportTask.class` next to it is the
 * exact bytes of the embedded base64 (decode-verify: the base64 of that file
 * equals the constant in rossiCommands.ts). See this directory's README.md for
 * how to regenerate both.
 *
 * Target: Java 17 (class file major version 61). Compiled against the Apache
 * Ant API and the Eclipse Platform resources/runtime APIs that Rodin bundles.
 */
package org.rossi.vscode;

import java.io.File;

import org.apache.tools.ant.BuildException;
import org.apache.tools.ant.Task;
import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IProjectDescription;
import org.eclipse.core.resources.IResource;
import org.eclipse.core.resources.IWorkspace;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.NullProgressMonitor;
import org.eclipse.core.runtime.Path;

public final class RodinProjectImportTask extends Task {
    private String projectDir;

    public void setProjectDir(String projectDir) {
        this.projectDir = projectDir;
    }

    @Override
    public void execute() throws BuildException {
        if (projectDir == null || projectDir.isBlank()) {
            throw new BuildException("projectDir is required");
        }
        try {
            File dir = new File(projectDir).getCanonicalFile();
            File dotProject = new File(dir, ".project");
            if (!dotProject.isFile()) {
                throw new BuildException("Missing .project file: " + dotProject);
            }

            IWorkspace workspace = ResourcesPlugin.getWorkspace();
            IProjectDescription description =
                workspace.loadProjectDescription(new Path(dotProject.getAbsolutePath()));
            IProject project = workspace.getRoot().getProject(description.getName());

            IProgressMonitor monitor = new NullProgressMonitor();
            if (!project.exists()) {
                project.create(description, monitor);
            }
            if (!project.isOpen()) {
                project.open(monitor);
            }
            project.refreshLocal(IResource.DEPTH_INFINITE, monitor);
            workspace.save(true, monitor);

            log("Imported Rodin project " + description.getName() + " from " + dir);
        } catch (BuildException e) {
            throw e;
        } catch (Exception e) {
            throw new BuildException(e);
        }
    }
}

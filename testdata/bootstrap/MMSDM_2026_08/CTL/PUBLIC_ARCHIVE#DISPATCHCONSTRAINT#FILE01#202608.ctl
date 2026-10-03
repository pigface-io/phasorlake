-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#DISPATCHCONSTRAINT#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DISPATCHCONSTRAINT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#DISPATCHCONSTRAINT#FILE01#202608010000.CSV
APPEND INTO TABLE DISPATCHCONSTRAINT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
RUNNO FLOAT EXTERNAL,
CONSTRAINTID CHAR(20),
DISPATCHINTERVAL FLOAT EXTERNAL,
INTERVENTION FLOAT EXTERNAL,
RHS FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL,
VIOLATIONDEGREE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
GENCONID_EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
GENCONID_VERSIONNO FLOAT EXTERNAL,
LHS FLOAT EXTERNAL)
-- End of Script
